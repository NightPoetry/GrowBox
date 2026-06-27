//! Memory 的分层检索面(设计/02):RAG 第一层 → 精确层 mesh 扫描/指针跟随/二级索引「远处拉近」+ 上下文组装。

use super::*;

impl Memory {
    /// 给尚未向量化(或向量空间已变)的节点补 embedding(后台/idle 调用)。
    /// 换 embedder → version 变 → 旧向量失效,这里整体重嵌(见 `embedding-service.md`)。
    pub async fn ensure_embeddings(&mut self, sub: &dyn Subconscious) {
        let ver = sub.embedding_version();
        // 待补 = 未向量化 或 版本不符(含旧数据空版本)。靠元信息筛(不触盘),只对待补节点惰性取 content。
        let todo: Vec<String> = self
            .timeline
            .metas()
            .iter()
            // 待破碎的大节点先别嵌(等破成块再嵌块);已破的父节点不嵌(它已退出索引)。
            .filter(|m| (!m.has_embedding || m.embedding_version != ver) && !m.needs_chunk && !m.chunked)
            .map(|m| m.id.clone())
            .collect();
        if todo.is_empty() {
            return;
        }
        for id in todo {
            let content = self.timeline.content(&id).unwrap_or_default(); // 文档原文用 passage 前缀
            let emb = sub.embed_passage(&content).await;
            self.timeline.set_embedding(&id, emb, ver.clone());
            self.persist_node(&id); // 向量也落库,重启后免重算
        }
        self.rebuild_index(); // 向量变了,重建第一层索引
    }

    /// 分批补 embedding(idle 飞轮调):每次最多嵌 `limit` 条待补节点,嵌完重建索引,返回本批实嵌条数。
    /// 返回 0 = 没有待补(可停)。比 `ensure_embeddings` 多了"有界 + 可在批间让位前台/取消"的好处
    /// ——避免一上来嵌成百上千条把一次 idle 卡死。`limit==0` 当作无界(=ensure_embeddings)。
    pub async fn ensure_embeddings_batch(&mut self, sub: &dyn Subconscious, limit: usize) -> usize {
        let ver = sub.embedding_version();
        let mut todo: Vec<String> = self
            .timeline
            .metas()
            .iter()
            // 待破碎的大节点先别嵌(等破成块再嵌块);已破的父节点不嵌(它已退出索引)。
            .filter(|m| (!m.has_embedding || m.embedding_version != ver) && !m.needs_chunk && !m.chunked)
            .map(|m| m.id.clone())
            .collect();
        if limit > 0 && todo.len() > limit {
            todo.truncate(limit);
        }
        if todo.is_empty() {
            return 0;
        }
        let done = todo.len();
        for id in todo {
            let content = self.timeline.content(&id).unwrap_or_default();
            let emb = sub.embed_passage(&content).await;
            self.timeline.set_embedding(&id, emb, ver.clone());
            self.persist_node(&id);
        }
        self.rebuild_index();
        done
    }

    // --- 文档破碎化(idle，排在补嵌之前) ---

    /// 分批破碎待破文档(idle 飞轮调,**排在补嵌之前**——好让生成的小块紧接着被嵌入):
    /// 每次最多破 `limit` 个待破节点,把每个按句破成小块(LLM 判破点 + 尺寸兜底)→ 各块独立成节点、
    /// 父节点标 `chunked` 退出索引,返回本批实破条数。返回 0 = 没有待破(可停)。
    /// 与 `ensure_embeddings_batch` 同构:有界 + 可在批间让位前台/取消。`limit==0` = 无界(破完所有待破)。
    pub async fn chunk_pending_batch(&mut self, sub: &dyn Subconscious, limit: usize) -> usize {
        // 待破 = meta.needs_chunk(只读元信息、不触盘)。
        let mut todo: Vec<String> = self
            .timeline
            .metas()
            .iter()
            .filter(|m| m.needs_chunk)
            .map(|m| m.id.clone())
            .collect();
        if limit > 0 && todo.len() > limit {
            todo.truncate(limit);
        }
        if todo.is_empty() {
            return 0;
        }
        let target = self.retrieval_cfg.chunk_min_chars; // 块目标尺寸 ≈ 破碎阈(块尽量大但不超嵌入窗)
        let mut done = 0usize;
        let mut any_chunked = false;
        for id in todo {
            let Some(content) = self.timeline.content(&id) else {
                self.timeline.clear_needs_chunk(&id); // 内容已不在(理论不会)→ 清标志免反复重试
                self.persist_node(&id);
                continue;
            };
            let role = self.timeline.meta(&id).map(|m| m.role.clone()).unwrap_or_else(|| "user".into());
            let project = self.timeline.meta(&id).and_then(|m| m.project_id.clone());
            let sentences = crate::memory::split_sentences(&content);
            let chunks = self.assemble_chunks(&sentences, sub, target).await;
            done += 1;
            if chunks.len() <= 1 {
                // 破不出第二块(单句超长 / 文档本就短小)→ 不产生重复子节点,仅清标志,父节点照常留用。
                self.timeline.clear_needs_chunk(&id);
                self.persist_node(&id);
                continue;
            }
            for c in chunks {
                self.ingest_chunk(c, role.clone(), project.clone());
            }
            self.timeline.mark_chunked(&id); // 父退出索引/补嵌/线性扫(内容已由块表示)
            self.persist_node(&id);
            any_chunked = true;
        }
        if any_chunked {
            self.rebuild_index(); // 已破父节点退出索引(块的向量由随后的补嵌 pass 再进)
        }
        done
    }

    /// 句序列 → 破点 → 各块原文(精确拼接,零丢字零改写)。破点 = LLM 判的语义"另起一块"句下标
    /// **并上**尺寸上限(任何块不超过 `target`,超了也断)——故 LLM 不给破点时退化成纯尺寸贪心,
    /// 绝不会又拼回一个大节点。返回各块原文(纯空白块丢弃)。
    /// `&mut self`(虽不改状态):使跨 `chunk_doc().await` 持有 `&mut Memory`(Send)而非 `&Memory`
    /// (因 RefCell !Sync 故 !Send),与其它 await 方法一致,可在 spawn 的 idle 循环里用。
    async fn assemble_chunks(&mut self, sentences: &[String], sub: &dyn Subconscious, target: usize) -> Vec<String> {
        if sentences.is_empty() {
            return Vec::new();
        }
        let llm_breaks: HashSet<usize> = sub
            .chunk_doc(sentences, target)
            .await
            .into_iter()
            .filter(|&i| i > 0 && i < sentences.len()) // 0 无意义(块本就从此起);越界丢弃
            .collect();
        let mut chunks = Vec::new();
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for (i, s) in sentences.iter().enumerate() {
            let len = s.chars().count();
            let llm_break = llm_breaks.contains(&i);
            let size_break = target > 0 && cur_len + len > target;
            if (llm_break || size_break) && !cur.trim().is_empty() {
                chunks.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            cur.push_str(s);
            cur_len += len;
        }
        if !cur.trim().is_empty() {
            chunks.push(cur);
        }
        chunks
    }

    // --- 检索:分层下沉 ---

    /// 分层检索。先 RAG;命中(相似度够)即返回;否则下沉精确层。
    /// 返回 (结果, 来自哪层)。
    pub async fn retrieve(&mut self, query: &str, sub: &dyn Subconscious) -> (Vec<Hit>, Layer) {
        // 第一层:RAG —— 走可切换的向量索引(ANN/暴力,见 `index.rs`),不再线性扫全表。
        let rcfg = self.retrieval_cfg;
        let qv = sub.embed_query(query).await;
        let mut rag: Vec<Hit> = self
            .index
            .search(&qv, rcfg.rag_topk)
            .into_iter()
            .filter_map(|(id, score)| {
                self.timeline.get(&id).map(|n| Hit { content: n.content.clone(), source: id, score })
            })
            .collect();
        // 项目软偏好(非硬过滤):命中属当前项目 → 相似度乘 (1+boost) 再重排。
        // 跨项目高相关仍可压过本项目低相关被召回(用户明确提别的项目/反复追问时自然扩大范围)。
        if let Some(cur) = self.current_project.as_deref() {
            if rcfg.project_boost > 0.0 {
                for h in &mut rag {
                    if self.timeline.meta(&h.source).and_then(|m| m.project_id.as_deref()) == Some(cur) {
                        h.score *= 1.0 + rcfg.project_boost;
                    }
                }
                rag.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            }
        }

        let (hits, layer) = if rag.first().map(|h| h.score >= rcfg.rag_hit_threshold).unwrap_or(false) {
            // RAG 命中:够用就停(反馈好与否由调用方决定,不好时再 retrieve_exact)
            rag.retain(|h| h.score >= rcfg.rag_hit_threshold);
            for h in &rag {
                self.timeline.touch(&h.source);
                self.persist_node(&h.source);
            }
            (rag, Layer::Rag)
        } else {
            // 下沉:第二层精确
            (self.retrieve_exact(query, sub).await, Layer::Exact)
        };

        // ② 检索入时间线(配套:自我感知第三次泛化)——把这次下沉检索记成**瞬态一等事件**,
        // 下回合经 render_internal_state 让 LLM 感知"我刚检索了什么、命中几条"。kind=mind_search(受控表)。
        // ring-only(不落时间线节点):检索是每回合高频动作,落节点会让 assemble_context 自我 perceive
        // 致上下文无界增长;只记摘要不记全遍历。
        let q_short: String = query.chars().take(40).collect();
        let layer_label = match layer {
            Layer::Rag => "RAG",
            Layer::Exact => "精确",
        };
        self.perceive_transient(
            crate::node_kind::MIND_SEARCH,
            format!("检索「{q_short}」:{layer_label}层,命中 {} 条", hits.len()),
        );
        (hits, layer)
    }

    /// 只召回项目级流程(process kind)的建议档:复用分层检索,再按 role 过滤到 process。
    /// 给 agent 在任务开始/触发时拉相关流程注入上下文(建议档供 AI 读、照做)。
    ///
    /// ★二期 B2 指针接通(越用越准)★:对过滤出的每条流程,经"流程召回边"(哨兵 → 流程)学习——
    /// ① 反 K 一票否决过的(同族 query 曾用它判错 / 它被更正版取代)→ **召回时滤掉**(误召的被压制);
    /// ② 留下的记正 K(召回 = 该 query 族在用)→ 累积 query 簇、heat 升,常对的流程越用越易被想起。
    /// 复用持久化 mesh 边(EDGES 表),零新存储(见 `02-process-kind落地.md` M2)。
    pub async fn retrieve_processes(&mut self, query: &str, sub: &dyn Subconscious) -> Vec<Hit> {
        let (hits, _layer) = self.retrieve(query, sub).await;
        let candidates: Vec<Hit> = hits
            .into_iter()
            .filter(|h| {
                self.timeline
                    .get(&h.source)
                    .map(|n| n.role == crate::node_kind::PROCESS)
                    .unwrap_or(false)
            })
            .collect();
        if candidates.is_empty() {
            return candidates;
        }
        // 流程召回学习需 query 向量;retrieve 内部已嵌但未回传,这里为学习重嵌一次(每任务起手仅一次)。
        let qv = sub.embed_query(query).await;
        let mut out = Vec::with_capacity(candidates.len());
        for h in candidates {
            if self.process_recall_vetoes(&h.source, &qv) {
                continue; // 反 K 否决:这族 query 曾用它判错 / 已被取代 → 不再浮现
            }
            self.reinforce_process_recall(&h.source, query, &qv); // 召回即正 K(该 query 族在用)
            out.push(h);
        }
        out
    }

    /// ★二期 B3 结晶(报告-纠正回路的写入半)★:把一条流程配方结晶成 process 节点(建议档),
    /// **即时嵌入 → 立刻可召回**(不等 idle 补向量)。`content` = "在本项目做 X = 碰 A→B→C" 配方原文。
    ///
    /// 近重复检测(推论4 持续合并 + 推论2 同一回路既建又修):把新配方当 query 搜已有流程节点,
    /// 若最相似一条 ≥ `PROCESS_MERGE_THRESHOLD`,视为"更正版取代旧版" → **对旧版召回边记反 K**
    /// (同族 query 以后规避旧版;append-only,旧节点不删)。返回 `(新节点 id, 被取代的旧流程 id)`。
    pub async fn crystallize_process(
        &mut self,
        content: impl Into<String>,
        sub: &dyn Subconscious,
    ) -> (String, Option<String>) {
        let content = content.into();
        // 近重复检测:新配方当 query 搜已有流程节点(索引存 passage 向量,查用 query 向量,同 retrieve)。
        let qv = sub.embed_query(&content).await;
        let supersede: Option<String> = self
            .index
            .search(&qv, PROCESS_MERGE_TOPK)
            .into_iter()
            .find(|(id, score)| {
                *score >= PROCESS_MERGE_THRESHOLD
                    && self.timeline.get(id).map(|n| n.role == crate::node_kind::PROCESS).unwrap_or(false)
            })
            .map(|(id, _)| id);
        // 写新节点 + 即时嵌入(passage)→ 重建索引,本节点立刻可被 retrieve_processes 召回。
        let pv = sub.embed_passage(&content).await;
        let new_id = self.ingest_process(content);
        let ver = sub.embedding_version();
        self.timeline.set_embedding(&new_id, pv, ver);
        self.persist_node(&new_id);
        self.rebuild_index();
        // 取代旧版:对旧版召回边记反 K(键=同族 query 向量),同族 query 以后规避它。
        if let Some(old) = &supersede {
            self.suppress_process_recall(old, &qv);
        }
        (new_id, supersede)
    }

    /// ★Skill 语义召回(设计/09 推论4 的「召回兜底」半)★:按 query 召回相关 skill 节点,
    /// 与 `retrieve_processes` 同构——反 K 否决误召、正 K 强化召回(越用越准)。清单(主动挑)是另一半,
    /// 在脊柱拼系统提示;两半结合。返回命中的 skill 节点 Hit(content = 结构化 playbook 全文)。
    pub async fn retrieve_skills(&mut self, query: &str, sub: &dyn Subconscious) -> Vec<Hit> {
        if !self.skill_cfg.enabled {
            return Vec::new(); // 总开关关:不召回 skill
        }
        let (hits, _layer) = self.retrieve(query, sub).await;
        let candidates: Vec<Hit> = hits
            .into_iter()
            .filter(|h| {
                self.timeline
                    .get(&h.source)
                    .map(|n| {
                        n.role == crate::node_kind::SKILL
                            // 停用的 skill 不召回(按名;skill 节点头解析出 name)
                            && crate::skill_format::parse_head(&n.content)
                                .map(|(name, _)| self.skill_cfg.is_active(&name))
                                .unwrap_or(true)
                    })
                    .unwrap_or(false)
            })
            .collect();
        if candidates.is_empty() {
            return candidates;
        }
        let qv = sub.embed_query(query).await;
        let mut out = Vec::with_capacity(candidates.len());
        for h in candidates {
            if self.skill_recall_vetoes(&h.source, &qv) {
                continue; // 反 K 否决:这族 query 曾用它判错 / 已被取代
            }
            self.reinforce_skill_recall(&h.source, query, &qv); // 召回即正 K
            out.push(h);
        }
        out
    }

    /// ★Skill 结晶(设计/09 推论7:即时学习 + 报告-纠正)★:把一个 skill(name/trigger/body)结晶成
    /// skill 节点,**即时嵌入 → 立刻可召回**。与 `crystallize_process` 同构:近重复(同名或高相似)
    /// 视为"更正版取代旧版" → 对旧版召回边记反 K(append-only,旧节点不删)。返回 `(新节点 id, 被取代旧 id)`。
    pub async fn crystallize_skill(
        &mut self,
        name: &str,
        trigger: &str,
        body: &str,
        sub: &dyn Subconscious,
    ) -> (String, Option<String>) {
        let content = crate::skill_format::format(name, trigger, body);
        // 取代判定:① 同名 skill 直接取代;② 否则按语义近重复(≥ 阈值)取代。
        let by_name = self.learned_skill_id_by_name(name);
        let qv = sub.embed_query(&content).await;
        let supersede: Option<String> = by_name.or_else(|| {
            self.index
                .search(&qv, PROCESS_MERGE_TOPK)
                .into_iter()
                .find(|(id, score)| {
                    *score >= PROCESS_MERGE_THRESHOLD
                        && self.timeline.get(id).map(|n| n.role == crate::node_kind::SKILL).unwrap_or(false)
                })
                .map(|(id, _)| id)
        });
        let pv = sub.embed_passage(&content).await;
        let new_id = self.ingest_skill(content);
        let ver = sub.embedding_version();
        self.timeline.set_embedding(&new_id, pv, ver);
        self.persist_node(&new_id);
        self.rebuild_index();
        if let Some(old) = &supersede {
            self.suppress_skill_recall(old, &qv);
        }
        (new_id, supersede)
    }

    /// 按名找已学 skill 节点 id(精确、大小写不敏感);供结晶取代判定。
    /// 取**最新**同名(append-only 后旧版仍在,反复结晶时取代应指向上一版而非最初版)。
    fn learned_skill_id_by_name(&self, name: &str) -> Option<String> {
        let mut latest = None;
        for m in self.timeline.metas() {
            if m.role != crate::node_kind::SKILL {
                continue;
            }
            if let Some(node) = self.timeline.get(&m.id) {
                if let Some((n, _)) = crate::skill_format::parse_head(&node.content) {
                    if n.eq_ignore_ascii_case(name) {
                        latest = Some(m.id.clone());
                    }
                }
            }
        }
        latest
    }

    // --- 工具记忆(计划/工具记忆-不犯第二遍:分发前会诊「小本本」)---

    /// ★工具记忆结晶★:写一条工具记忆节点 + **即时嵌入**(立刻可被分发前会诊命中,不等 idle)。
    /// 无显式 supersede:会诊取"最相似且最新"一条(同情况的新记录凭更新的 created_at 在并列相似度时
    /// 胜出,自然反映关键因素变化后的覆盖)。返回新节点 id。
    pub async fn crystallize_tool_memory(
        &mut self,
        tool: &str,
        situation: &str,
        verdict: crate::tool_memory_format::Verdict,
        detail: &str,
        sub: &dyn Subconscious,
    ) -> String {
        let content = crate::tool_memory_format::format(tool, situation, verdict, detail);
        let pv = sub.embed_passage(&content).await;
        let id = self.ingest_tool_memory(content);
        let ver = sub.embedding_version();
        self.timeline.set_embedding(&id, pv, ver);
        self.persist_node(&id);
        self.rebuild_index();
        id
    }

    /// ★分发前会诊(设计 B)★:当前要调 `tool`、情况 `situation`,查本工具(本项目)已记工具记忆里
    /// **最相似且最新**的一条 → 返回 `(verdict, 记忆全文, 相似度)`。无匹配 / 本工具无记忆 → None。
    /// 嵌入 query 后对本工具各记忆直接算余弦(条数少;成本门 `tool_memory_count` 应在上层先 gate)。
    /// 排序:相似度降序,并列取 created_at 最新(新记录覆盖旧结论 = 关键因素变化的自校正)。
    /// `&mut self`(虽不改状态):使跨 `embed_query().await` 持有的是 `&mut Memory`(Send)而非
    /// `&Memory`(因 RefCell !Sync 故 !Send),与其它 retrieve_* 一致,可在 spawn 的 agent 循环里用。
    pub async fn consult_tool_memory(
        &mut self,
        tool: &str,
        situation: &str,
        sub: &dyn Subconscious,
    ) -> Option<(crate::tool_memory_format::Verdict, String, f32)> {
        let qv = sub.embed_query(situation).await;
        let cur = self.current_project.as_deref();
        // recency 用 metas 插入序(下标越大越新),不靠 created_at —— 同毫秒两次 ingest 时间戳会撞,
        // 插入序天然单调,稳。tiebreak:相似度并列时取更新的(新记录覆盖旧结论 = 关键因素变化自校正)。
        let mut cands: Vec<(crate::tool_memory_format::Verdict, String, f32, usize)> = Vec::new();
        for (idx, m) in self.timeline.metas().iter().enumerate() {
            if m.role != crate::node_kind::TOOL_MEMORY || !m.has_embedding {
                continue;
            }
            if !super::project_visible(&m.project_id, cur) {
                continue;
            }
            let Some(node) = self.timeline.get(&m.id) else { continue };
            if node.embedding.is_empty() {
                continue;
            }
            let Some((t, _situ, v)) = crate::tool_memory_format::parse_head(&node.content) else { continue };
            if !t.eq_ignore_ascii_case(tool) {
                continue;
            }
            let cos = crate::subconscious::cosine(&qv, &node.embedding);
            cands.push((v, node.content, cos, idx));
        }
        cands.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.3.cmp(&a.3))
        });
        cands.into_iter().next().map(|(v, c, cos, _)| (v, c, cos))
    }

    /// 强制走精确层(用于"RAG 反馈不好"或"用户引用历史")。
    ///
    /// 网状飞轮(`设计/02` 推论1+2):时间线是主干路,指针是架其上的快车道。步骤:
    /// 步骤一 找入口:取 embedding 相似度最高的前 K 个节点(进图的门)。
    /// 步骤一·半 强制跳转(阶段4):入口落到带强制跳转指针的位置 → 无条件召回其目标(用户引用的历史)。
    /// 步骤二 走快车道:沿入口出边里 topic 匹配 qv 的边,跳到关联目标(前沿);并把二级索引(阶段4)里
    /// 热的远端 target 拉近注入前沿。只 judge 前沿这几个——即便目标已染 Deep 也能经边召回。
    /// 步骤三 快车道无果 → 沿主干路从近往远线性扫(跳 Deep),judge 一批;
    /// 线性命中则从入口(或当前位置)向命中目标建一条边,网由此生长。
    pub async fn retrieve_exact(&mut self, query: &str, sub: &dyn Subconscious) -> Vec<Hit> {
        let rcfg = self.retrieval_cfg;
        let qv = sub.embed_query(query).await;
        let current_len = self.timeline.len();

        // ── 步骤1:找入口(向量索引取 top-K 节点作进图的门) ──
        let entries: Vec<String> = self
            .index
            .search(&qv, rcfg.entry_k)
            .into_iter()
            .filter(|(_, s)| *s >= rcfg.entry_min_sim)
            .map(|(id, _)| id)
            .collect();

        // ── 步骤1.5:强制跳转指针(位置键,遍历到入口即必跳,无 topic 门、无需 judge) ──
        // 用户曾在某位置引用历史并钉下目标;只要导航入口落到该位置,就无条件召回那段历史。
        let mut forced_hits: Vec<Hit> = Vec::new();
        let mut surfaced: HashSet<String> = HashSet::new();
        for e in &entries {
            for tgt in self.forced_jumps_of(e) {
                if tgt == *e || !surfaced.insert(tgt.clone()) {
                    continue;
                }
                if let Some(content) = self.timeline.content(&tgt) {
                    if self.timeline.meta(&tgt).map(|m| m.stain) == Some(Stain::None) {
                        self.timeline.stain(&tgt, Stain::Light); // 快扫召回
                    }
                    self.timeline.touch(&tgt);
                    self.persist_node(&tgt);
                    forced_hits.push(Hit { content, source: tgt, score: 1.0 });
                }
            }
        }

        // ── 步骤2:沿快车道跳到前沿(入口出边里 topic 匹配的 target)+ 二级索引拉近 ──
        let mut frontier: Vec<(String, String)> = Vec::new(); // (target_id, content)
        let mut edge_source: HashMap<String, String> = HashMap::new(); // target -> 带该边的入口(一级 source)
        let mut pulled: HashSet<String> = HashSet::new(); // 来自二级索引(非 topic 边)的 target
        let mut edge_hits: Vec<(String, String)> = Vec::new(); // (entry,target):跟随过的边,judge 后回填正/负 K
        let cfg = self.pointer_cfg;
        for e in &entries {
            for p in self.edges_of(e) {
                // 反 K 一票否决(两档共用):这条 query 曾在此 lane 误跳过 → 规避(省 judge/省 LLM)。
                if p.neg_veto(&qv, cfg.neg_block_threshold) {
                    continue;
                }
                let Some(content) = self.timeline.get(&p.target).map(|n| n.content.clone()) else {
                    continue; // target 无内容(已删/不在线):不进前沿、不学习
                };
                // 跟随判定按匹配档:档A 廉价加权余弦;档B 把正负 K 原文喂 LLM 综合判断(精确、贵)。
                let follow = match cfg.match_mode {
                    PointerMatchMode::WeightedCosine => p.follow_score(&qv, cfg.weight_gain) >= cfg.follow_threshold,
                    PointerMatchMode::LlmJudge => {
                        let pos: Vec<String> = p
                            .positives
                            .iter()
                            .filter(|k| !k.text.is_empty())
                            .map(|k| format!("{}(命中{}次)", k.text, k.weight))
                            .collect();
                        let neg: Vec<String> = p
                            .negatives
                            .iter()
                            .filter(|k| !k.text.is_empty())
                            .map(|k| k.text.clone())
                            .collect();
                        sub.judge_edge(query, &pos, &neg, &content).await
                    }
                };
                if !follow {
                    continue;
                }
                edge_hits.push((e.clone(), p.target.clone())); // 归因:这条边跟随了本次 query
                if frontier.iter().any(|(id, _)| id == &p.target) {
                    continue; // 同 target 已在前沿(他入口的边);上面已记 edge_hits,不重复 judge
                }
                edge_source.entry(p.target.clone()).or_insert_with(|| e.clone());
                frontier.push((p.target.clone(), content));
            }
        }
        // 二级索引「远处拉近」:把热的远端 target 注入前沿——即便当前入口的 topic 边够不着它
        // (原始边根在已漂远的旧 source),也能凭锚到前沿的二级锚点被召回。
        for tgt in self.secondary.pull_targets(SECONDARY_PULL_N) {
            if surfaced.contains(&tgt) || frontier.iter().any(|(id, _)| id == &tgt) {
                continue;
            }
            if let Some(content) = self.timeline.content(&tgt) {
                pulled.insert(tgt.clone());
                frontier.push((tgt, content));
            }
        }
        if !frontier.is_empty() {
            // force_judge_on_cosine_hit=false 且档A:余弦命中的边 target 直接采纳(省一次前沿 judge);
            // 二级索引「远处拉近」的推测项(pulled)不是余弦命中,无论此旋钮如何仍需 judge 确认。
            let relevant: Vec<usize> = if cfg.match_mode == PointerMatchMode::WeightedCosine
                && !cfg.force_judge_on_cosine_hit
            {
                let mut rel: Vec<usize> = Vec::new();
                let mut to_judge: Vec<usize> = Vec::new();
                for (i, (id, _)) in frontier.iter().enumerate() {
                    if pulled.contains(id) {
                        to_judge.push(i); // 二级拉近的推测项仍需确认
                    } else {
                        rel.push(i); // 档A 余弦命中,直接采纳
                    }
                }
                if !to_judge.is_empty() {
                    let cands: Vec<String> = to_judge.iter().map(|&i| frontier[i].1.clone()).collect();
                    for j in sub.judge_relevant(query, &cands).await {
                        if let Some(&i) = to_judge.get(j) {
                            rel.push(i);
                        }
                    }
                }
                rel
            } else {
                let candidates: Vec<String> = frontier.iter().map(|(_, c)| c.clone()).collect();
                sub.judge_relevant(query, &candidates).await
            };
            let relevant_ids: HashSet<String> = relevant
                .iter()
                .filter_map(|&i| frontier.get(i).map(|(id, _)| id.clone()))
                .collect();
            let mut hits = forced_hits;
            for (i, (id, content)) in frontier.iter().enumerate() {
                if !relevant.contains(&i) || surfaced.contains(id) {
                    continue;
                }
                if pulled.contains(id) {
                    self.secondary.bump(id); // 经二级锚点召回,加热(边的正 K 由下方 edge_hits 学习)
                }
                // 经边到达染 Light(快扫可能漏);已 Deep 不降级(染色单调升)。
                if self.timeline.meta(id).map(|m| m.stain) == Some(Stain::None) {
                    self.timeline.stain(id, Stain::Light);
                }
                self.timeline.touch(id);
                self.persist_node(id);
                // 二级索引「远处拉近」维护:命中的远端 target 漂离前沿 ≥ K 窗口则锚到当前前沿
                // (K→2K 粗化,见 `secondary.rs`);近端的 register 会自动撤锚。
                let l1_source = edge_source
                    .get(id)
                    .cloned()
                    .or_else(|| entries.first().cloned())
                    .unwrap_or_else(|| id.clone());
                if let Some(pos) = self.timeline.position(id) {
                    let windows_away = current_len.saturating_sub(pos) / WINDOW;
                    self.secondary.register(id, &l1_source, windows_away, current_len);
                }
                // 欠债记账(P5):走快车道/二级锚点直达 target,跳过了入口与 target 之间的中段 = 碎片。
                // 记一笔债,做梦(`dream_once`)来复查中段有无遗漏。
                if let Some(entry) = entries.first() {
                    if entry != id {
                        self.fragments.record(entry, id, query, qv.clone());
                    }
                }
                hits.push(Hit { content: content.clone(), source: id.clone(), score: 1.0 });
            }
            // ── 学习更新(阶段2):topic 边匹配过的 target,judge 相关→记正 K(累积去重+heat),
            // judge 拒→记反 K(硬负样本)。替代旧"复用即全量 bump_edge",并回填此前被丢弃的拒绝信息。
            for (src, tgt) in &edge_hits {
                if surfaced.contains(tgt) {
                    continue; // 强制跳转召回的目标不参与边学习
                }
                if relevant_ids.contains(tgt) {
                    self.record_positive_edge(src, tgt, query, &qv);
                } else {
                    self.record_negative_edge(src, tgt, query, &qv);
                }
            }
            if !hits.is_empty() {
                return dedup_hits(hits);
            }
            // 前沿都不相关:指针失效,不删(append-only),落到线性兜底。
        } else if !forced_hits.is_empty() {
            // 无前沿但强制跳转已召回(用户引用的历史)→ 直接返回,不必再线性扫。
            return dedup_hits(forced_hits);
        }

        // ── 步骤3:线性主干路(★核心:从最新位置往回的渐进召回,设计/02 "翻到最早=全量扫描" 的有界实现)★ ──
        // ★2026-06-15 核心修复★:旧版只扫最近 SCAN_BATCH 个就停 + 把扫过的一律染 Deep → 未嵌入的新记忆
        // (idle 补嵌前)被一次不相关扫描永久踢出线性兜底,而步骤1/2入口又全靠嵌入索引够不着它 → 检索盲区
        // (置换率恒0 + 上下文割裂,dream-board CSS 漏接即此类)。现在:
        //   ① 从最新往回**多批渐进扫**,直到攒够 target 命中 / 扫满 scan_max / 扫到最早(不做"连续空批早停"
        //      —— 早停会把"埋在一堆无关记忆下的相关旧契约"漏掉,违反"刚产生=刚可检索"的基本假设;
        //      成本控制交给染色[跳已确认区]+ scan_max[硬上限],这才忠于设计/02 "翻到最早=全量扫描");
        //   ② **染色分级**:有嵌入(索引这条退路在)才染 Deep 跳过;未嵌入只染 Light,保持线性可扫,
        //      直到 idle 把它嵌进索引后才安全 Deep —— 护住基本假设(靠 L2,不靠猛嵌入)。
        let target = rcfg.rag_topk.max(1); // 攒够这么多条命中即够,早停省 judge
        let scan_max = rcfg.scan_max.max(rcfg.scan_batch); // 本次最多线性读多少节点(有界"全量")
        // 从最新往回收集非 Deep 的 id(只读元信息、不触盘);content 仅对真扫到的批惰性取。
        let scan_ids: Vec<String> = self
            .timeline
            .metas()
            .iter()
            .rev()
            .filter(|m| m.stain != Stain::Deep && !m.chunked) // 已破父节点跳过(内容已由各块表示)
            .map(|m| m.id.clone())
            .collect();
        // 建边的源:最相关的入口;无入口(RAG 全空)则用当前位置(最近节点)。
        let anchor: Option<String> = entries
            .first()
            .cloned()
            .or_else(|| self.timeline.metas().last().map(|m| m.id.clone()));
        let mut hits = Vec::new();
        let mut scanned = 0usize;
        for chunk in scan_ids.chunks(rcfg.scan_batch.max(1)) {
            if scanned >= scan_max || hits.len() >= target {
                break;
            }
            let batch: Vec<(String, String)> = chunk
                .iter()
                .filter_map(|id| self.timeline.content(id).map(|c| (id.clone(), c)))
                .collect();
            if batch.is_empty() {
                continue;
            }
            let candidates: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let relevant = sub.judge_relevant(query, &candidates).await;
            for (i, (id, content)) in batch.iter().enumerate() {
                scanned += 1;
                // 染色分级:有嵌入(索引退路在)→ Deep(确信扫过、可安全跳过);未嵌入 → Light(保持线性可扫)。
                let has_emb = self.timeline.meta(id).map(|m| m.has_embedding).unwrap_or(false);
                self.timeline
                    .stain(id, if has_emb { Stain::Deep } else { Stain::Light });
                if relevant.contains(&i) {
                    self.timeline.touch(id);
                    // 网生长:从入口/当前位置 → 命中目标建一条关联边(键=本次 query 向量)。
                    if let Some(src) = anchor.clone() {
                        if src != *id {
                            self.link_edge(&src, id, query, qv.clone());
                        }
                    }
                    hits.push(Hit { content: content.clone(), source: id.clone(), score: 1.0 });
                }
                self.persist_node(id);
            }
        }
        hits
    }

    /// P4 上下文组装:把无界记忆映射进有限上下文窗口,产出 llm 无关的 `ContextBlock` 序列
    /// (稳定→易变:工作记忆区 → 8K 最近 ring)。system 与当前回合由 gui 拼。
    ///
    /// - 工作记忆区:跑分层检索,命中项按指针/相似度**调入常驻工作集**(两态去重,见 `context.rs`);
    ///   已常驻的不重复拼,只刷新热度。
    /// - 8K 最近 ring:取时间线尾部、在 ring 预算内的最近若干节点;**与工作区去重**(ring 覆盖近因)。
    /// - 每块带完整时间戳(工作区非线性,gui 提示词按时间戳判先后)。
    pub async fn assemble_context(&mut self, query: &str, sub: &dyn Subconscious) -> Vec<ContextBlock> {
        // ① 检索 → 调入工作记忆区(两态)。RAG/L2 找到的都进**同一存放区**,只是来源标签不同:
        //    RAG(ANN 直接跳、无扫描路径)= 假指针 RagFake(无序列位置、换出不落序列、不进碎片);
        //    Exact(L2 顺序扫)= 真指针 Llm(序列位置/二级锚/碎片回收在 retrieve_exact)。
        let (hits, layer) = self.retrieve(query, sub).await;
        let origin = match layer {
            Layer::Rag => Origin::RagFake,
            Layer::Exact => Origin::Llm,
        };
        for h in &hits {
            let (role, ts, heat) = self
                .timeline
                .meta(&h.source)
                .map(|m| (m.role.clone(), m.created_at, m.hits))
                .unwrap_or_else(|| ("system".to_string(), growbox_core::now(), 0));
            self.context.page_in_with_origin(h.source.clone(), role, ts, h.content.clone(), heat, origin);
        }

        // ② 8K 最近 ring:从时间线尾部往前,装到 ring 预算满为止(最新在最后)。
        let ring_ids = self.recent_ring_ids();

        // ③ 组装:工作区(排除 ring 内 id,不重复) + ring。
        let mut blocks = self.context.working_blocks(&ring_ids);
        for id in &ring_ids {
            if let (Some(content), Some(meta)) = (self.timeline.content(id), self.timeline.meta(id)) {
                blocks.push(ContextBlock {
                    region: Region::RecentRing,
                    node_id: id.clone(),
                    role: meta.role.clone(),
                    timestamp: meta.created_at,
                    content,
                });
            }
        }
        blocks
    }

    /// ★回合内补检索(in-loop supplementary retrieval)★:任务进行到一半,AI 才发现需要某信息
    /// (例:开始 SSH 才意识到要用户名/密码),而上下文里没有——因为开场的 `assemble_context` 只按
    /// **进场那一句用户消息**检索过一次。本方法让脊柱在循环里**用 AI 当下的思路/进展作新查询再检索一次**,
    /// 把新命中的长期记忆**增量**调入工作区。
    ///
    /// 与 `assemble_context` 的区别:① 不组 8K ring(ring 由开场一次性给,循环内新事件靠 append-only 注入,
    /// 不在此重复);② **只返回本次"真新调入"的块**——已常驻的经 `is_resident` 去重(仅刷新热度、不重复拼),
    /// 故调用方据此决定是否追加一段"补充记忆"system 消息(无新命中则返回空、不打扰)。append-only 渲染保 KV 前缀稳。
    /// 检索层判定(RAG→精确下沉)、置换计数(`page_in`)、`mind_search` 自我感知均复用 `retrieve`,与开场一致。
    pub async fn supplement_context(&mut self, query: &str, sub: &dyn Subconscious) -> Vec<ContextBlock> {
        let (hits, layer) = self.retrieve(query, sub).await;
        let origin = match layer {
            Layer::Rag => Origin::RagFake,
            Layer::Exact => Origin::Llm,
        };
        let mut fresh = Vec::new();
        for h in &hits {
            // 去重必须在 page_in 之前判:page_in 会把命中变成常驻,之后再判恒为真。
            // 已常驻 → page_in 仅刷新热度(原位不动),不收作新块;未常驻 → 调入 + 收作"真新块"回给调用方渲染。
            let already = self.context.is_resident(&h.source);
            let (role, ts, heat) = self
                .timeline
                .meta(&h.source)
                .map(|m| (m.role.clone(), m.created_at, m.hits))
                .unwrap_or_else(|| ("system".to_string(), growbox_core::now(), 0));
            self.context
                .page_in_with_origin(h.source.clone(), role.clone(), ts, h.content.clone(), heat, origin);
            if !already {
                fresh.push(ContextBlock {
                    region: Region::Working,
                    node_id: h.source.clone(),
                    role,
                    timestamp: ts,
                    content: h.content.clone(),
                });
            }
        }
        fresh
    }
}

/// 按 source 去重,保留首次出现顺序(强制跳转 + mesh 前沿可能召回同一节点)。
fn dedup_hits(hits: Vec<Hit>) -> Vec<Hit> {
    let mut seen = HashSet::new();
    hits.into_iter().filter(|h| seen.insert(h.source.clone())).collect()
}
