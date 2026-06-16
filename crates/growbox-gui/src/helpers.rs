//! OS 授权 helper app 体系 —— "疫苗式"持久授权。
//!
//! 思路(用户 2026-06-09):需要持久 OS 授权(TCC 自动化等)的能力,做成 `GrowBox.app/Contents/Helpers/`
//! 下的**签名小 app**,每个有自己稳定的代码签名身份 → 自己独立的 TCC 授权。GrowBox 按需 spawn:
//!   - 探针(probe)= 做一次无害的同类动作触发系统授权弹窗(疫苗),用户允许一次即永久。
//!   - 执行 = 复用该授权干真活;经 `open` 由 LaunchServices 以 helper 自身身份启动,detached、
//!     存活过 GrowBox 退出(故"定时 / 退出后"也用同一授权,见关机)。
//! 裸 .sh/.scpt 拿不到自己独立的持久 TCC 授权(无签名身份),故必须是签名的 .app(随主 app 一起签)。
//! 第一个 helper = ShutdownHelper(System Events 自动化关机,免 root)。

use std::path::PathBuf;
use std::process::Command;

/// 解析 helper app 路径:`GrowBox.app/Contents/Helpers/<name>.app`。
/// 非打包运行(cargo run,exe 不在 Contents/MacOS 下)或未构建 helper → None。
pub fn helper_app(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?; // .../GrowBox.app/Contents/MacOS/growbox
    let contents = exe.parent()?.parent()?; // .../Contents
    let app = contents.join("Helpers").join(format!("{name}.app"));
    app.is_dir().then_some(app)
}

/// helper 是否已随包装好(决定走 helper 还是回退老路)。
pub fn helper_exists(name: &str) -> bool {
    helper_app(name).is_some()
}

/// 经 LaunchServices 以 helper 自身身份启动它(detached,存活过 GrowBox 退出;TCC 归属 helper 自己)。
/// args 经 `open --args` 传给 helper 的 `on run argv`。
pub fn launch_helper(name: &str, args: &[&str]) -> Result<(), String> {
    let app = helper_app(name).ok_or_else(|| format!("未找到 helper「{name}」(非打包运行或未构建)"))?;
    let mut cmd = Command::new("open");
    cmd.arg("-a").arg(&app).arg("--args").args(args);
    cmd.spawn().map(|_| ()).map_err(|e| format!("启动 helper「{name}」失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_helper_resolves_none() {
        // 测试环境(cargo test)exe 不在 .app bundle 里 → 找不到 helper,优雅返回 None / false。
        assert!(helper_app("ShutdownHelper").is_none());
        assert!(!helper_exists("ShutdownHelper"));
        assert!(launch_helper("ShutdownHelper", &["probe"]).is_err());
    }
}
