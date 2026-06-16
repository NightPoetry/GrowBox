//! 沙箱判定。

use growbox_core::Risk;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// 待判定的操作。
pub enum Operation<'a> {
    /// 读路径。
    Read(&'a Path),
    /// 写路径。
    Write(&'a Path),
    /// 执行 shell 命令。
    Shell(&'a str),
    /// 出站网络访问(完整 URL,web_fetch/web_search 用)。
    Net(&'a str),
}

/// 判定结果。
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// 放行,直接执行。
    Allow,
    /// 越界,需把裁决交还用户(带原因)。
    NeedAuth { reason: String },
    /// 硬拒绝(命中永久黑名单,授权也不放行)。
    Deny { reason: String },
}

/// 授权范围(用户点选,见 `设计/03` 推论3)。
#[derive(Debug, Clone, PartialEq)]
pub enum GrantScope {
    /// 仅这一次。
    Once,
    /// 本项目内这个路径,持久。
    ThisProjectPath(PathBuf),
    /// 本项目内允许访问这个内网/本机主机(host 小写,不含端口),持久。
    /// 与路径授权各自独立——网络授权绝不放宽文件访问,反之亦然。
    ThisProjectHost(String),
    /// 本项目内放开此类,持久(最宽)。
    ThisProject,
}

/// 沙箱:路径分级 + 黑名单 + 已授权记录。
pub struct Sandbox {
    writable_roots: Vec<PathBuf>,
    readonly_roots: Vec<PathBuf>,
    /// 用户已授权的路径(持久授权,本项目级)。
    granted_paths: Vec<PathBuf>,
    /// 用户已授权的内网/本机主机(小写,不含端口;持久授权,本项目级)。
    granted_hosts: Vec<String>,
    /// 用户已对本项目整体授权放开。
    project_granted: bool,
    /// ★danger 模式(为所欲为)★:用户在全自动之上显式开启的最高放行档。开启后 `judge` 一律放行
    /// (绕过越界/敏感路径/危险命令/SSRF 全部门),让无人值守的自驱能做系统级操作(系统装 Python、
    /// 全局 npm 等)而不卡在授权弹窗。**极高风险**,仅供用户明知故犯地短期开启。会话级、不持久。
    danger: bool,
}

impl Sandbox {
    pub fn new(writable: Vec<PathBuf>, readonly: Vec<PathBuf>) -> Self {
        Sandbox {
            writable_roots: canon_all(writable),
            readonly_roots: canon_all(readonly),
            granted_paths: Vec::new(),
            granted_hosts: Vec::new(),
            project_granted: false,
            danger: false,
        }
    }

    /// 切换 danger 模式(为所欲为)。由 app 按 `Settings.danger_mode` 每回合同步(见 cmds::run_chat /
    /// set_danger_mode)。开启后所有 `judge` 放行——调用方务必确认这是用户的明确选择。
    pub fn set_danger(&mut self, on: bool) {
        self.danger = on;
    }

    /// 记录一次用户授权。
    pub fn grant(&mut self, scope: GrantScope) {
        match scope {
            GrantScope::Once => {} // 单次授权由调用方一次性放行,不持久化
            GrantScope::ThisProjectPath(p) => self.granted_paths.push(canon(&p)),
            GrantScope::ThisProjectHost(h) => self.granted_hosts.push(h.to_ascii_lowercase()),
            GrantScope::ThisProject => self.project_granted = true,
        }
    }

    /// 判定一个操作。
    pub fn judge(&self, op: &Operation) -> Verdict {
        // ★danger 模式★:为所欲为——一律放行(绕过越界/敏感路径/危险命令/SSRF 全部门)。
        // 用户在全自动之上明确开启的最高档,让无人值守自驱能做系统级操作不卡授权。极高风险。
        if self.danger {
            return Verdict::Allow;
        }
        match op {
            Operation::Read(p) => self.judge_read(p),
            Operation::Write(p) => self.judge_write(p),
            Operation::Shell(cmd) => self.judge_shell(cmd),
            Operation::Net(url) => self.judge_net(url),
        }
    }

    fn judge_read(&self, path: &Path) -> Verdict {
        let c = canon(path);
        if let Some(reason) = sensitive_hit(&c) {
            return Verdict::Deny { reason };
        }
        if self.project_granted
            || self.is_under(&c, &self.writable_roots)
            || self.is_under(&c, &self.readonly_roots)
            || self.is_granted(&c)
        {
            return Verdict::Allow;
        }
        Verdict::NeedAuth {
            reason: format!("读取 {} 不在项目可访问范围内", c.display()),
        }
    }

    fn judge_write(&self, path: &Path) -> Verdict {
        let c = canon(path);
        if let Some(reason) = sensitive_hit(&c) {
            return Verdict::Deny { reason };
        }
        if self.project_granted || self.is_under(&c, &self.writable_roots) || self.is_granted(&c) {
            return Verdict::Allow;
        }
        // 只读目录里写 → 越界,交还裁决
        Verdict::NeedAuth {
            reason: format!("写入 {} 不在可写范围内", c.display()),
        }
    }

    fn judge_shell(&self, cmd: &str) -> Verdict {
        let lc = cmd.trim().to_lowercase();
        // 危险命令前缀:硬拒绝(sudo / rm -rf / / 磁盘工具等)。
        for bad in DANGEROUS_COMMANDS {
            if lc.starts_with(bad) {
                return Verdict::Deny {
                    reason: format!("命令命中危险前缀 '{}'", bad.trim()),
                };
            }
        }
        // 引用敏感密钥/凭据路径(.ssh / id_rsa / .env / credentials 等):交还裁决。
        // 关键修正(用户反馈 2026-06-02):**可写目录是"文件读写范围",不是"shell 可引用路径白名单"**。
        // 运行标准位置的程序(/usr/bin、/opt/homebrew/bin、/bin 下的 node/python 等)是正常开发操作,
        // 不应因此要授权。旧逻辑用子串匹配 `/bin/` 把 `/opt/homebrew/bin/node` 也误判越界,导致跑任何
        // 程序都被拦——shell 只该被"危险命令黑名单 + 敏感密钥路径"约束,不被系统路径白名单约束。
        let env_secret = references_env_secret(&lc);
        for pat in SENSITIVE_PATHS {
            // .env 特例:排除 .env.example/.sample/.template/.dist 等公开模板(约定俗成会进仓库,非密钥),
            // 也排除 .environment 等非 env 文件;真 .env / .env.local / .env.production 仍拦。其余模式照旧子串匹配。
            let hit = if *pat == "/.env" { env_secret } else { lc.contains(pat) };
            if hit {
                if self.project_granted {
                    continue; // 用户已项目级信任 shell
                }
                return Verdict::NeedAuth {
                    reason: format!("命令引用了敏感路径 '{pat}'"),
                };
            }
        }
        Verdict::Allow
    }

    /// 出站网络判定(web_fetch/web_search,见 `设计/03`):
    /// - 非 http(s) → 硬拒(file:///etc/passwd、gopher 等协议走私,授权也不放行);
    /// - 内网/本机地址(localhost / 私网 IP / .local 等)→ 交还用户裁决(SSRF 面;
    ///   调试本地 dev server 是正当用法,授权即放行;已授权主机直接过);
    /// - 公网 → 放行(只读取;结果按不可信外部输入标注,与 MCP 同)。
    ///
    /// 注意:**不**消费 `project_granted`——网络授权与文件授权互不放宽(见 GrantScope 注释)。
    fn judge_net(&self, url: &str) -> Verdict {
        let (_scheme, host, _port) = match parse_http_url(url) {
            Ok(t) => t,
            Err(reason) => return Verdict::Deny { reason },
        };
        if host_is_private_literal(&host) {
            if self.granted_hosts.iter().any(|g| g == &host) {
                return Verdict::Allow;
            }
            return Verdict::NeedAuth {
                reason: format!("访问内网/本机地址 {host} 需要你的确认(防服务器侧请求伪造)"),
            };
        }
        Verdict::Allow
    }

    fn is_under(&self, path: &Path, roots: &[PathBuf]) -> bool {
        roots.iter().any(|r| path.starts_with(r))
    }
    fn is_granted(&self, path: &Path) -> bool {
        self.granted_paths.iter().any(|r| path.starts_with(r))
    }
}

// --- 网络判定工具函数(web 执行器复用同一真源做 DNS rebinding 复查) ---

/// 解析 http(s) URL → (scheme, host 小写, port)。只接受 http/https;其余 scheme 给出拒绝原因。
/// 手写极简解析(safety 不引 url crate):剥 scheme://、剥 userinfo@、识别 [IPv6]、剥 :port 与 /path。
pub fn parse_http_url(url: &str) -> Result<(String, String, u16), String> {
    let u = url.trim();
    let (scheme, rest) = match u.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => return Err(format!("URL 缺少协议(应为 http:// 或 https://): {u}")),
    };
    let default_port: u16 = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        other => return Err(format!("不支持的协议 '{other}'(只允许 http/https)")),
    };
    // authority = 到第一个 /、?、# 为止;剥 userinfo(防 `http://safe.com@127.0.0.1/` 混淆)。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return Err(format!("URL 缺少主机名: {u}"));
    }
    // [IPv6]:port / [IPv6] / host:port / host
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((h, p)) => {
                let port = match p.strip_prefix(':') {
                    Some(ps) => ps.parse::<u16>().map_err(|_| format!("端口非法: {ps}"))?,
                    None => default_port,
                };
                (h.to_ascii_lowercase(), port)
            }
            None => return Err(format!("IPv6 主机缺少 ']': {authority}")),
        }
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        // 纯 IPv6 无括号(罕见且不合 URL 规范)会含多个 ':',此处只按最后一个切;parse 失败按整体 host。
        match p.parse::<u16>() {
            Ok(port) => (h.to_ascii_lowercase(), port),
            Err(_) => (authority.to_ascii_lowercase(), default_port),
        }
    } else {
        (authority.to_ascii_lowercase(), default_port)
    };
    if host.is_empty() {
        return Err(format!("URL 缺少主机名: {u}"));
    }
    Ok((scheme, host, port))
}

/// 主机名**字面上**是否指向内网/本机(不做 DNS):IP 字面量按网段分级;
/// localhost/*.localhost/*.local/*.internal/*.lan/*.home.arpa 等本地域按名分级。
/// 公网域名解析到内网(DNS rebinding)由执行器在解析后用 [`ip_is_private`] 复查。
pub fn host_is_private_literal(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    for suffix in [".local", ".internal", ".lan", ".home.arpa", ".intranet"] {
        if h.ends_with(suffix) {
            return true;
        }
    }
    // 无点裸主机名(NetBIOS/mDNS 风格,如 http://router/)只可能由本地解析 → 按内网处理。
    if !h.contains('.') && h.parse::<IpAddr>().is_err() {
        return true;
    }
    if let Ok(ip) = h.parse::<IpAddr>() {
        return ip_is_private(&ip);
    }
    false
}

/// IP 是否属于内网/本机/特殊段(loopback/私网/链路本地/CGNAT/未指定/广播/ULA 等)。
/// 执行器对"公网域名解析出的 IP"复查(DNS rebinding),与 judge 用同一真源。
pub fn ip_is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()                          // 127.0.0.0/8
                || v4.is_private()                    // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()                 // 169.254/16(含云元数据 169.254.169.254)
                || v4.is_unspecified()                // 0.0.0.0
                || v4.is_broadcast()                  // 255.255.255.255
                || o[0] == 100 && (64..128).contains(&o[1]) // 100.64/10 CGNAT
                || o[0] == 192 && o[1] == 0 && o[2] == 0    // 192.0.0.0/24 IETF
        }
        IpAddr::V6(v6) => {
            // IPv4 映射(::ffff:a.b.c.d)按内嵌 v4 判。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_is_private(&IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()                          // ::1
                || v6.is_unspecified()                // ::
                || (seg[0] & 0xfe00) == 0xfc00        // fc00::/7 ULA
                || (seg[0] & 0xffc0) == 0xfe80        // fe80::/10 链路本地
        }
    }
}

/// 把不可逆操作的风险等级纳入判定:Risk::Irreversible 即使路径合法也要确认。
/// 由 app 在 dispatch 时结合 `Executor::risk()` 调用。
pub fn risk_gate(risk: Risk, path_verdict: Verdict) -> Verdict {
    match (risk, &path_verdict) {
        (Risk::Irreversible, Verdict::Allow) => Verdict::NeedAuth {
            reason: "该操作不可逆,请确认".to_string(),
        },
        _ => path_verdict,
    }
}

// --- 黑名单与工具函数 ---

fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        // 不存在的目标:canonicalize 父目录 + 拼文件名,防 `../` 逃逸
        match path.parent().and_then(|p| std::fs::canonicalize(p).ok()) {
            Some(parent) => parent.join(path.file_name().unwrap_or_default()),
            None => path.to_path_buf(),
        }
    })
}

fn canon_all(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().map(|p| canon(&p)).collect()
}

/// 命中敏感路径 → Some(原因)。
fn sensitive_hit(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    let env_secret = references_env_secret(&s);
    for pat in SENSITIVE_PATHS {
        // .env 特例同 judge_shell:公开模板/非 env 文件不算密钥(见 references_env_secret)。
        let hit = if *pat == "/.env" { env_secret } else { s.contains(pat) };
        if hit {
            return Some(format!("路径命中敏感模式 '{pat}'"));
        }
    }
    None
}

/// `.env` 公开模板后缀(约定俗成会提交进仓库,非密钥)。
const ENV_PUBLIC_SUFFIXES: &[&str] = &["example", "sample", "template", "dist", "md", "txt"];

/// 字符串里是否引用了**真正的 .env 密钥文件**——排除 `.env.example/.sample/.template/.dist`(公开模板)
/// 与 `.environment` 等非 env 文件;`.env` / `.env.local` / `.env.production` 等仍算密钥。
/// 匹配 `/.env` 路径片段,据其后紧邻字符判定:分隔/结束=真;`.公开后缀`=模板放行;`.其它`=真;字母数字=非 env 文件。
fn references_env_secret(s: &str) -> bool {
    let needle = "/.env";
    let mut from = 0;
    while let Some(rel) = s[from..].find(needle) {
        let after = from + rel + needle.len();
        from = after; // 下次从这之后继续找
        let rest = &s[after..];
        match rest.chars().next() {
            None => return true, // 以 /.env 结尾 → 真密钥
            Some(c) if c.is_alphanumeric() => continue, // .environment 等 → 非 .env 文件
            Some('.') => {
                let suffix: String = rest[1..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase();
                if !ENV_PUBLIC_SUFFIXES.contains(&suffix.as_str()) {
                    return true; // .env.local / .env.production 等 → 真密钥
                }
                // 公开模板(.env.example 等):继续找下一处
            }
            Some(_) => return true, // 分隔符/引号/斜杠等 → 真 .env
        }
    }
    false
}

const SENSITIVE_PATHS: &[&str] = &[
    "/.ssh", "/.gnupg", "/.aws", "/.config/gcloud", "/credentials", "/.env", "/id_rsa", "/id_ed25519",
];

const DANGEROUS_COMMANDS: &[&str] = &[
    "sudo ", "su ", "rm -rf /", "chmod 777", "chown -r ", "mount ", "umount ", "fdisk ", "mkfs",
    "dd if=", "shutdown", "reboot", "init ", "systemctl ", "launchctl ", "networksetup ", "dscl ",
    "defaults write", "csrutil ", "nvram ", "diskutil ", "hdiutil ",
];

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sb_with(writable: &Path, readonly: &Path) -> Sandbox {
        Sandbox::new(vec![writable.to_path_buf()], vec![readonly.to_path_buf()])
    }

    #[test]
    fn write_in_writable_allowed() {
        let dir = tempdir().unwrap();
        let w = dir.path().join("rw");
        let r = dir.path().join("ro");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        let sb = sb_with(&w, &r);
        assert_eq!(sb.judge(&Operation::Write(&w.join("a.txt"))), Verdict::Allow);
    }

    #[test]
    fn write_in_readonly_needs_auth() {
        let dir = tempdir().unwrap();
        let w = dir.path().join("rw");
        let r = dir.path().join("ro");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        let sb = sb_with(&w, &r);
        let v = sb.judge(&Operation::Write(&r.join("a.txt")));
        assert!(matches!(v, Verdict::NeedAuth { .. }));
    }

    #[test]
    fn read_in_readonly_allowed() {
        let dir = tempdir().unwrap();
        let w = dir.path().join("rw");
        let r = dir.path().join("ro");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        let sb = sb_with(&w, &r);
        assert_eq!(sb.judge(&Operation::Read(&r.join("a.txt"))), Verdict::Allow);
    }

    #[test]
    fn outside_needs_auth() {
        let dir = tempdir().unwrap();
        let w = dir.path().join("rw");
        std::fs::create_dir_all(&w).unwrap();
        let sb = Sandbox::new(vec![w.clone()], vec![]);
        let v = sb.judge(&Operation::Read(Path::new("/tmp")));
        assert!(matches!(v, Verdict::NeedAuth { .. }));
    }

    #[test]
    fn grant_path_then_allowed() {
        let dir = tempdir().unwrap();
        let w = dir.path().join("rw");
        let extra = dir.path().join("assets");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let mut sb = Sandbox::new(vec![w], vec![]);
        assert!(matches!(sb.judge(&Operation::Write(&extra.join("x"))), Verdict::NeedAuth { .. }));
        sb.grant(GrantScope::ThisProjectPath(extra.clone()));
        assert_eq!(sb.judge(&Operation::Write(&extra.join("x"))), Verdict::Allow);
    }

    #[test]
    fn sensitive_path_denied_even_if_granted() {
        let dir = tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let mut sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        sb.grant(GrantScope::ThisProject);
        let v = sb.judge(&Operation::Read(&ssh.join("id_rsa")));
        assert!(matches!(v, Verdict::Deny { .. }), "敏感路径即使授权也拒绝");
    }

    #[test]
    fn dangerous_command_denied() {
        let sb = Sandbox::new(vec![], vec![]);
        assert!(matches!(sb.judge(&Operation::Shell("sudo rm -rf /")), Verdict::Deny { .. }));
        assert!(matches!(sb.judge(&Operation::Shell("diskutil eraseDisk x")), Verdict::Deny { .. }));
    }

    #[test]
    fn danger_mode_allows_everything_then_restores() {
        // ★danger 模式(为所欲为)★:越界/敏感/危险命令/内网 全放行;关掉即恢复硬底线。
        let dir = tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let mut sb = Sandbox::new(vec![], vec![]);
        sb.set_danger(true);
        assert_eq!(sb.judge(&Operation::Write(Path::new("/etc/hosts"))), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Read(&ssh.join("id_rsa"))), Verdict::Allow, "danger 连敏感路径也放行");
        assert_eq!(sb.judge(&Operation::Shell("sudo apt install python3")), Verdict::Allow, "danger 放行 sudo");
        assert_eq!(sb.judge(&Operation::Net("http://169.254.169.254/")), Verdict::Allow, "danger 放行内网");
        sb.set_danger(false);
        assert!(matches!(sb.judge(&Operation::Shell("sudo apt install python3")), Verdict::Deny { .. }), "关掉 danger 硬底线复位");
    }

    #[test]
    fn safe_command_allowed() {
        let sb = Sandbox::new(vec![], vec![]);
        assert_eq!(sb.judge(&Operation::Shell("ls -la")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Shell("cargo build 2>/dev/null")), Verdict::Allow);
    }

    #[test]
    fn binary_path_not_treated_as_whitelist() {
        // 修复(用户反馈 2026-06-02):运行标准位置的程序属正常,不因引用 /bin、/usr/bin、
        // /opt/homebrew/bin 而要授权(旧逻辑子串匹配 /bin/ 把 /opt/homebrew/bin/node 误判越界)。
        let sb = Sandbox::new(vec![], vec![]);
        assert_eq!(sb.judge(&Operation::Shell("cd /proj && /opt/homebrew/bin/node app.js")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Shell("/usr/bin/python3 -V")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Shell("npm run build")), Verdict::Allow);
    }

    #[test]
    fn sensitive_secret_path_in_shell_needs_auth() {
        // 敏感密钥/凭据路径仍交还裁决(这才是 shell 该约束的)。
        let sb = Sandbox::new(vec![], vec![]);
        assert!(matches!(sb.judge(&Operation::Shell("cat ~/.ssh/id_rsa")), Verdict::NeedAuth { .. }));
    }

    #[test]
    fn env_template_is_not_a_secret() {
        // 真机反馈(2026-06-09):写 .env.example(公开模板,约定俗成进仓库)被 `/.env` 子串误伤要授权。
        // 公开模板放行;真 .env / .env.local / .env.production 仍拦;.environment 等非 env 文件放行。
        let sb = Sandbox::new(vec![], vec![]);
        assert_eq!(
            sb.judge(&Operation::Shell("cat > /proj/.env.example << EOF")),
            Verdict::Allow,
            ".env.example 是公开模板,不该要授权"
        );
        assert_eq!(sb.judge(&Operation::Shell("cp /proj/.env.sample /proj/.env.example")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Shell("touch /proj/.environment.ts")), Verdict::Allow, ".environment 非 env 文件");
        assert!(matches!(sb.judge(&Operation::Shell("cat /proj/.env")), Verdict::NeedAuth { .. }), "真 .env 仍拦");
        assert!(matches!(sb.judge(&Operation::Shell("cat /proj/.env.local")), Verdict::NeedAuth { .. }), ".env.local 是密钥");
        assert!(matches!(sb.judge(&Operation::Shell("cat /proj/.env.production")), Verdict::NeedAuth { .. }));
        // 文件路径分类同理(file_write 目标命中)。
        assert!(references_env_secret("/proj/.env"));
        assert!(references_env_secret("/proj/.env.local"));
        assert!(!references_env_secret("/proj/.env.example"));
        assert!(!references_env_secret("/proj/.environment"));
    }

    #[test]
    fn irreversible_risk_forces_auth() {
        // 即使路径合法,不可逆操作也要确认。
        let v = risk_gate(Risk::Irreversible, Verdict::Allow);
        assert!(matches!(v, Verdict::NeedAuth { .. }));
        // 可逆操作不受影响。
        assert_eq!(risk_gate(Risk::Reversible, Verdict::Allow), Verdict::Allow);
    }

    // --- 出站网络判定(web_fetch/web_search 的 SSRF 面) ---

    #[test]
    fn net_public_url_allowed() {
        let sb = Sandbox::new(vec![], vec![]);
        assert_eq!(sb.judge(&Operation::Net("https://docs.rs/reqwest")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Net("http://example.com:8080/a?q=1")), Verdict::Allow);
    }

    #[test]
    fn net_private_or_local_needs_auth() {
        let sb = Sandbox::new(vec![], vec![]);
        for url in [
            "http://localhost:3000/",
            "http://127.0.0.1:8080/key",
            "https://192.168.1.50/admin",
            "http://10.0.0.5/",
            "http://172.16.1.1/",
            "http://169.254.169.254/latest/meta-data/", // 云元数据
            "http://[::1]:9999/",
            "http://router/", // 无点裸主机名只可能本地解析
            "http://nas.local/",
        ] {
            assert!(
                matches!(sb.judge(&Operation::Net(url)), Verdict::NeedAuth { .. }),
                "{url} 应交还用户裁决"
            );
        }
    }

    #[test]
    fn net_non_http_denied() {
        let sb = Sandbox::new(vec![], vec![]);
        for url in ["file:///etc/passwd", "ftp://example.com/x", "gopher://x/", "no-scheme-at-all"] {
            assert!(matches!(sb.judge(&Operation::Net(url)), Verdict::Deny { .. }), "{url} 应硬拒");
        }
    }

    #[test]
    fn net_granted_host_then_allowed_without_widening_files() {
        let dir = tempdir().unwrap();
        let mut sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        assert!(matches!(sb.judge(&Operation::Net("http://localhost:3000/")), Verdict::NeedAuth { .. }));
        sb.grant(GrantScope::ThisProjectHost("localhost".into()));
        assert_eq!(sb.judge(&Operation::Net("http://localhost:3000/")), Verdict::Allow);
        assert_eq!(sb.judge(&Operation::Net("http://LOCALHOST:9999/other")), Verdict::Allow, "host 不区分大小写/端口");
        // 主机授权不放宽别的内网主机,也不放宽文件访问。
        assert!(matches!(sb.judge(&Operation::Net("http://192.168.1.1/")), Verdict::NeedAuth { .. }));
        assert!(matches!(sb.judge(&Operation::Read(Path::new("/tmp"))), Verdict::NeedAuth { .. }));
    }

    #[test]
    fn net_project_grant_does_not_open_network() {
        // 文件侧"信任本项目"不放宽网络(互不放宽,见 GrantScope 注释)。
        let mut sb = Sandbox::new(vec![], vec![]);
        sb.grant(GrantScope::ThisProject);
        assert!(matches!(sb.judge(&Operation::Net("http://localhost:3000/")), Verdict::NeedAuth { .. }));
    }

    #[test]
    fn parse_http_url_handles_forms() {
        assert_eq!(parse_http_url("https://Docs.RS/path").unwrap(), ("https".into(), "docs.rs".into(), 443));
        assert_eq!(parse_http_url("http://a.com:81/x?y#z").unwrap(), ("http".into(), "a.com".into(), 81));
        assert_eq!(parse_http_url("http://[::1]:8080/").unwrap(), ("http".into(), "::1".into(), 8080));
        assert_eq!(parse_http_url("http://[2001:db8::1]/").unwrap(), ("http".into(), "2001:db8::1".into(), 80));
        // userinfo 混淆:`safe.com@127.0.0.1` 的真实主机是 127.0.0.1。
        assert_eq!(parse_http_url("http://safe.com@127.0.0.1/").unwrap().1, "127.0.0.1");
        assert!(parse_http_url("file:///etc/passwd").is_err());
        assert!(parse_http_url("http://").is_err());
    }

    #[test]
    fn host_and_ip_classification() {
        for h in ["localhost", "a.localhost", "nas.local", "x.internal", "box.lan", "h.home.arpa", "router", "127.0.0.1", "10.1.2.3", "172.31.0.1", "192.168.0.9", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1"] {
            assert!(host_is_private_literal(h), "{h} 应判内网/本机");
        }
        for h in ["example.com", "docs.rs", "api.search.brave.com", "8.8.8.8", "1.1.1.1", "2606:4700::1111", "172.32.0.1", "100.128.0.1"] {
            assert!(!host_is_private_literal(h), "{h} 应判公网");
        }
        // DNS rebinding 复查用同一真源:IPv4 映射 IPv6 内嵌私网照样识别。
        assert!(ip_is_private(&"::ffff:192.168.1.1".parse().unwrap()));
        assert!(!ip_is_private(&"::ffff:8.8.8.8".parse().unwrap()));
    }
}
