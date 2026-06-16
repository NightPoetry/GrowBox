//! 本地嵌入 —— candle 跑 `multilingual-e5-small`(纯 Rust CPU,跨平台无原生依赖)。
//!
//! 实现 `计划/embedding-service.md` 阶段2 + `打包设计.md` 的模型解析:
//! 1. resource_dir/models/<name>(带模型包预置)
//! 2. data_dir/models/<name>(上次下载缓存)
//! 3. hf-hub 下载到 data_dir/models/<name>(不带模型包,首次联网)
//!
//! e5 必须加前缀(实测召回坑):查询 `query: `,文档 `passage: `。池化用带 mask 的均值 + L2 归一化。
//! 模型懒加载(首次 embed 时按上面顺序解析/下载);加载失败则该次返回错误,RAG 退化到精确层。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::Tokenizer;
use tokio::sync::Mutex;

use crate::embed::{EmbedKind, Embedder};
use crate::error::{LlmError, LlmResult};

/// 内嵌的默认本地模型(用户决策 2026-05-31)。
const MODEL_NAME: &str = "multilingual-e5-small";
const HF_REPO: &str = "intfloat/multilingual-e5-small";

/// 已加载的模型件(model + tokenizer 都是 Send+Sync,可跨 await 持有)。
struct Loaded {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

/// 懒加载状态:没试过 / 失败(不再重试,避免每次 embed 都重下)/ 已就绪。
enum LoadState {
    Untried,
    Failed(String),
    Ready(Arc<Loaded>),
}

/// 本地 e5 嵌入器。持有解析配置,首次 embed 时加载。
pub struct LocalE5Embedder {
    /// 按序探测的模型根目录(各目录下找 `<MODEL_NAME>/{config,tokenizer,model}`)。
    search_dirs: Vec<PathBuf>,
    /// 探测全落空时,下载落地到此根目录下的 `<MODEL_NAME>/`。
    download_root: PathBuf,
    state: Mutex<LoadState>,
}

impl LocalE5Embedder {
    /// `search_dirs`:优先级从高到低的模型根目录(如 [resource_dir/models, data_dir/models])。
    /// `download_root`:全落空时下载到此(通常 data_dir/models)。
    pub fn new(search_dirs: Vec<PathBuf>, download_root: PathBuf) -> Self {
        LocalE5Embedder { search_dirs, download_root, state: Mutex::new(LoadState::Untried) }
    }

    /// 在某根目录下找齐三件套则返回其目录。
    fn resolved_dir(root: &Path) -> Option<PathBuf> {
        let dir = root.join(MODEL_NAME);
        let ok = ["config.json", "tokenizer.json", "model.safetensors"]
            .iter()
            .all(|f| dir.join(f).is_file());
        ok.then_some(dir)
    }

    /// 解析模型目录:先探 search_dirs;全落空则 hf-hub 下载到 download_root 并复制成统一布局。
    async fn ensure_model_dir(&self) -> LlmResult<PathBuf> {
        for root in &self.search_dirs {
            if let Some(dir) = Self::resolved_dir(root) {
                return Ok(dir);
            }
        }
        // 下载(不带模型包路径):取 config/tokenizer/weights,复制到 download_root/<name>/。
        let dest = self.download_root.join(MODEL_NAME);
        std::fs::create_dir_all(&dest).map_err(|e| LlmError::Config(format!("建模型目录失败: {e}")))?;
        let api = hf_hub::api::tokio::Api::new()
            .map_err(|e| LlmError::Config(format!("hf-hub 初始化失败: {e}")))?;
        let repo = api.model(HF_REPO.to_string());
        for f in ["config.json", "tokenizer.json", "model.safetensors"] {
            let src = repo
                .get(f)
                .await
                .map_err(|e| LlmError::Config(format!("下载 {f} 失败: {e}")))?;
            std::fs::copy(&src, dest.join(f))
                .map_err(|e| LlmError::Config(format!("复制 {f} 失败: {e}")))?;
        }
        Ok(dest)
    }

    /// 真正加载(同步:读文件 + 建模型)。
    fn load_from_dir(dir: &Path) -> LlmResult<Loaded> {
        let device = Device::Cpu;
        let config: Config = {
            let s = std::fs::read_to_string(dir.join("config.json"))
                .map_err(|e| LlmError::Config(format!("读 config 失败: {e}")))?;
            serde_json::from_str(&s).map_err(|e| LlmError::Parse(format!("解析 config 失败: {e}")))?
        };
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| LlmError::Config(format!("加载 tokenizer 失败: {e}")))?;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DTYPE, &device)
                .map_err(|e| LlmError::Config(format!("mmap 权重失败: {e}")))?
        };
        let model = BertModel::load(vb, &config)
            .map_err(|e| LlmError::Config(format!("建 BERT 失败: {e}")))?;
        Ok(Loaded { model, tokenizer, device })
    }

    /// 取已加载模型(懒加载 + 缓存;失败缓存不再重试)。
    async fn get_loaded(&self) -> LlmResult<Arc<Loaded>> {
        let mut state = self.state.lock().await;
        match &*state {
            LoadState::Ready(m) => return Ok(m.clone()),
            LoadState::Failed(e) => return Err(LlmError::Config(format!("本地嵌入模型不可用: {e}"))),
            LoadState::Untried => {}
        }
        let result = match self.ensure_model_dir().await {
            Ok(dir) => Self::load_from_dir(&dir),
            Err(e) => Err(e),
        };
        match result {
            Ok(loaded) => {
                let arc = Arc::new(loaded);
                *state = LoadState::Ready(arc.clone());
                Ok(arc)
            }
            Err(e) => {
                *state = LoadState::Failed(e.to_string());
                Err(e)
            }
        }
    }

    /// 单条:前缀 → 分词 → forward → 带 mask 均值池化 → L2 归一化。
    fn embed_one(loaded: &Loaded, text: &str, kind: EmbedKind) -> LlmResult<Vec<f32>> {
        let prefix = match kind {
            EmbedKind::Query => "query: ",
            EmbedKind::Passage => "passage: ",
        };
        let input = format!("{prefix}{text}");
        let enc = loaded
            .tokenizer
            .encode(input, true)
            .map_err(|e| LlmError::Parse(format!("分词失败: {e}")))?;
        let ids = enc.get_ids();
        let mask = enc.get_attention_mask();
        let dev = &loaded.device;

        let to_err = |e: candle_core::Error| LlmError::Parse(format!("张量运算失败: {e}"));
        let token_ids = Tensor::new(ids, dev).map_err(to_err)?.unsqueeze(0).map_err(to_err)?; // [1, seq]
        let token_type_ids = token_ids.zeros_like().map_err(to_err)?;
        // attention mask 传 f32:bert 内部对其做 (1-mask)*MIN 浮点运算,u32 会运行时 dtype 报错。
        let mask_f = Tensor::new(mask, dev)
            .map_err(to_err)?
            .to_dtype(DTYPE)
            .map_err(to_err)?
            .unsqueeze(0)
            .map_err(to_err)?; // [1, seq]

        let out = loaded
            .model
            .forward(&token_ids, &token_type_ids, Some(&mask_f))
            .map_err(to_err)?; // [1, seq, hidden]

        // 带 mask 的均值池化:对有效 token 求和 / 有效 token 数(复用同一个 f32 mask)。
        let mask_exp = mask_f.unsqueeze(2).map_err(to_err)?; // [1, seq, 1]
        let summed = out.broadcast_mul(&mask_exp).map_err(to_err)?.sum(1).map_err(to_err)?; // [1, hidden]
        let count = mask_f.sum(1).map_err(to_err)?.unsqueeze(1).map_err(to_err)?; // [1, 1]
        let mean = summed
            .broadcast_div(&count)
            .map_err(to_err)?
            .squeeze(0)
            .map_err(to_err)?
            .contiguous()
            .map_err(to_err)?; // [hidden],连续以便 to_vec1

        // L2 归一化(余弦相似度前置)。
        let norm = mean.sqr().map_err(to_err)?.sum_all().map_err(to_err)?.sqrt().map_err(to_err)?;
        let norm = norm.to_scalar::<f32>().map_err(to_err)?;
        let v: Vec<f32> = mean.to_vec1().map_err(to_err)?;
        if norm <= 0.0 {
            return Ok(v);
        }
        Ok(v.into_iter().map(|x| x / norm).collect())
    }
}

#[async_trait]
impl Embedder for LocalE5Embedder {
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> LlmResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let loaded = self.get_loaded().await?;
        // candle CPU 推理是同步计算;逐条算(batch 优化留待上量后做)。
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(Self::embed_one(&loaded, t, kind)?);
        }
        Ok(out)
    }

    fn version(&self) -> String {
        format!("local:{MODEL_NAME}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机验证(需联网下载 ~470MB e5 模型):
    /// `cargo test -p growbox-llm live_e5 -- --ignored --nocapture`
    /// 验"同义不同词"相似度高于无关——词法版做不到,这是换真 embedding 的核心目的。
    #[tokio::test]
    #[ignore]
    async fn live_e5_synonym_recall() {
        let dir = std::env::temp_dir().join("growbox_e5_test_model");
        let emb = LocalE5Embedder::new(vec![], dir); // 空 search → 直接 hf-hub 下载
        let q = emb.embed(&["如何重启路由器".into()], EmbedKind::Query).await.unwrap();
        let related = emb.embed(&["路由器怎么重新启动".into()], EmbedKind::Passage).await.unwrap();
        let unrelated = emb.embed(&["今天晚饭吃什么好呢".into()], EmbedKind::Passage).await.unwrap();
        assert_eq!(q[0].len(), 384, "multilingual-e5-small 应为 384 维");
        // 向量已 L2 归一化,点积即余弦。
        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let sim_related = cos(&q[0], &related[0]);
        let sim_unrelated = cos(&q[0], &unrelated[0]);
        println!("related={sim_related:.4} unrelated={sim_unrelated:.4}");
        assert!(sim_related > sim_unrelated, "同义应比无关更相似");
    }
}
