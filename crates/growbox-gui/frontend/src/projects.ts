// 项目系统服务：刷新列表、切换、新建、目录增删。
// 与 v1 frontend/index.html:644-880 等价语义。

import { api, listen } from "./tauri-api";
import {
  setProjects,
  setCurrentProjectId,
  setProjectDirectories,
  setProjectCreateOpen,
  setAddPathOpen,
  setProjectDropdownOpen,
  projectDirectories,
  flashingPaths,
  setFlashingPaths,
  runtimeDir,
} from "./store";
import { notify } from "./notices";

/// 离线引导：未连 LM 时也能浏览项目/历史。
/// 用 localStorage 里的 runtime_dir 调 set_runtime_dir → 后端初始化 ProjectManager →
/// 前端 refreshProjects/refreshProjectDirs 立刻拿到真实数据；用户后续 connect 时
/// connect 会覆盖 runtime_dir + rebuild，这里只是"预热"不会冲突。
export async function bootstrapOffline(): Promise<void> {
  const dir = runtimeDir().trim();
  if (!dir) return;
  try {
    await api.setRuntimeDir(dir);
    await refreshProjects();
    await refreshProjectDirs();
  } catch (e) {
    // 静默失败:多数情况是 runtime_dir 路径无写权限,toast 太吵
    console.warn("[bootstrap] setRuntimeDir failed:", e);
  }
}

interface DirAccessPayload {
  path?: string;
  kind?: string;
  write?: boolean;
}

// 监听后端 emit_dir_access_event → pd-row 闪 1.5s
// path → "write" | "read" 进入 flashingPaths，setTimeout 后清除
let dirAccessUnlisten: (() => void) | null = null;
export async function startDirAccessListener(): Promise<void> {
  if (dirAccessUnlisten) return;
  dirAccessUnlisten = await listen<DirAccessPayload>("dir-access", (payload) => {
    const path = payload.path;
    if (!path) return;
    const kind: "write" | "read" = payload.write ? "write" : "read";
    setFlashingPaths({ ...flashingPaths(), [path]: kind });
    setTimeout(() => {
      const cur = { ...flashingPaths() };
      if (cur[path] === kind) {
        delete cur[path];
        setFlashingPaths(cur);
      }
    }, 1500);
  });
}

export async function refreshProjects(): Promise<void> {
  try {
    const list = await api.listProjects();
    setProjects(list);
    const cur = await api.currentProject();
    setCurrentProjectId(cur);
  } catch (e) {
    notify("project.list_read_failed", { detail: String(e) });
  }
}

export async function refreshProjectDirs(): Promise<void> {
  try {
    const dirs = await api.getProjectDirectories(null);
    setProjectDirectories(dirs);
  } catch {
    setProjectDirectories(null);
  }
}

export async function switchToProject(id: string): Promise<void> {
  try {
    const info = await api.switchProject(id) as { id: string; name: string };
    setCurrentProjectId(info.id);
    setProjectDropdownOpen(false);
    notify("project.switched", { name: info.name });
    await refreshProjects();
    await refreshProjectDirs();
  } catch (e) {
    notify("project.switch_failed", { detail: String(e) });
  }
}

export async function createProject(args: {
  id: string; name: string; writable: string[]; readonly: string[]; description?: string;
}): Promise<boolean> {
  try {
    // 后端返回实际的项目 ID（用户指定的或自动生成的）
    const actualId = await api.createProject(args);
    setProjectCreateOpen(false);
    await refreshProjects();
    // 用后端确认的 id 切换，不用前端猜测的 args.id
    await switchToProject(actualId);
    notify("project.created", { name: args.name });
    return true;
  } catch (e) {
    notify("project.create_failed", { detail: String(e) });
    return false;
  }
}

export async function addPath(kind: "writable" | "readonly", path: string): Promise<boolean> {
  const dirs = projectDirectories();
  if (!dirs) {
    notify("project.none_active");
    return false;
  }
  const writable = [...dirs.writable];
  const readonly = [...dirs.readonly];
  if (kind === "writable") writable.push(path);
  else readonly.push(path);
  try {
    await api.updateProjectDirectories({ id: dirs.id, writable, readonly });
    setAddPathOpen(false);
    await refreshProjectDirs();
    notify(kind === "writable" ? "project.dir_added_writable" : "project.dir_added_readonly");
    return true;
  } catch (e) {
    notify("project.dir_add_failed", { detail: String(e) });
    return false;
  }
}

export async function movePath(fromKind: "writable" | "readonly", toKind: "writable" | "readonly", path: string): Promise<boolean> {
  const dirs = projectDirectories();
  if (!dirs) {
    notify("project.none_active");
    return false;
  }
  let writable = [...dirs.writable];
  let readonly = [...dirs.readonly];
  if (fromKind === "writable") writable = writable.filter((p) => p !== path);
  else readonly = readonly.filter((p) => p !== path);
  if (toKind === "writable") writable.push(path);
  else readonly.push(path);
  if (writable.length === 0) {
    notify("project.min_one_writable");
    return false;
  }
  try {
    await api.updateProjectDirectories({ id: dirs.id, writable, readonly });
    setAddPathOpen(false);
    await refreshProjectDirs();
    notify(toKind === "writable" ? "project.dir_moved_to_writable" : "project.dir_moved_to_readonly");
    return true;
  } catch (e) {
    notify("project.dir_move_failed", { detail: String(e) });
    return false;
  }
}

export async function removePath(kind: "writable" | "readonly", path: string): Promise<void> {
  const dirs = projectDirectories();
  if (!dirs) return;
  let writable = [...dirs.writable];
  let readonly = [...dirs.readonly];
  if (kind === "writable") writable = writable.filter((p) => p !== path);
  else readonly = readonly.filter((p) => p !== path);
  if (kind === "writable" && writable.length === 0) {
    notify("project.min_one_writable");
    return;
  }
  try {
    await api.updateProjectDirectories({ id: dirs.id, writable, readonly });
    await refreshProjectDirs();
    notify("project.dir_removed");
  } catch (e) {
    notify("project.dir_remove_failed", { detail: String(e) });
  }
}
