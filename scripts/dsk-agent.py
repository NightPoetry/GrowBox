#!/usr/bin/env python3
"""DeepSeek V4-pro 体力活执行器 —— 给模型装上 read/write/edit/run 工具,让它在本地循环执行机械任务,完事出报告。

定位:Opus 出判断与审查,本脚本让 v4-pro 干被明确指定的机械改动(删文件、改引用、跑构建)。
禁止用于复杂模块/架构设计——那是 Opus 的事。

用法:
  DEEPSEEK_API_KEY=xxx python3 scripts/dsk-agent.py \
      --task task.md --cwd <repo根> --report report.md [--max-steps 40] [--model deepseek-v4-pro]

设计要点:
  - 非流式;deepseek 是推理模型(先 reasoning_content 后 content),max_tokens 给足防截断。
  - 工具:read_file / write_file / edit_file / list_dir / grep / run / finish。
  - 全程 transcript 落盘(--report 同名 .log),供 Opus 审查;finish 的报告写 --report。
  - 路径限制在 --cwd 内,run 也在 cwd 执行;不做联网/危险命令白名单(本机授权场景)。
"""
import argparse, json, os, subprocess, sys, urllib.request, urllib.error

API_URL = "https://api.deepseek.com/chat/completions"
OUT_CAP = 8000  # 单个工具结果回灌上限,防上下文爆

SYSTEM = """你是体力活执行器,只做被明确指定的机械改动(删文件、改引用、跑构建、grep 自查)。
铁律:
1. 绝不做架构设计、复杂逻辑、新模块——遇到需要设计的地方,停下,在 finish 报告里写明"需 Opus 决策",不要自作主张。
2. 只动任务里明确点名的文件;其它文件一律不碰。
3. 每一步都用工具推进,不要只输出文字。
4. 改完必须用 run 跑任务里的构建判据验证;用 grep 自查没有残留引用。
5. 完成或卡住时调用 finish,报告:改了哪些文件、每个文件做了什么、构建结果、哪些不确定/没动。
禁止 Emoji。"""

def tools_spec():
    s = lambda **p: {"type": "object", "properties": p, "required": [k for k in p]}
    return [
        {"type": "function", "function": {"name": "read_file", "description": "读文件(可带行范围)", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "start": {"type": "integer"}, "end": {"type": "integer"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "write_file", "description": "整体写文件(覆盖)", "parameters": s(path={"type": "string"}, content={"type": "string"})}},
        {"type": "function", "function": {"name": "edit_file", "description": "把文件里唯一的 old 子串替换为 new", "parameters": s(path={"type": "string"}, old={"type": "string"}, new={"type": "string"})}},
        {"type": "function", "function": {"name": "delete_file", "description": "删除文件", "parameters": s(path={"type": "string"})}},
        {"type": "function", "function": {"name": "list_dir", "description": "列目录", "parameters": s(path={"type": "string"})}},
        {"type": "function", "function": {"name": "grep", "description": "在路径下递归搜正则(忽略 node_modules/target)", "parameters": s(pattern={"type": "string"}, path={"type": "string"})}},
        {"type": "function", "function": {"name": "run", "description": "在 cwd 跑 shell 命令,返回 stdout+stderr(截断)", "parameters": s(cmd={"type": "string"})}},
        {"type": "function", "function": {"name": "finish", "description": "提交最终报告并结束", "parameters": s(report={"type": "string"})}},
    ]

def safe(cwd, path):
    p = os.path.realpath(os.path.join(cwd, path))
    if not p.startswith(os.path.realpath(cwd)):
        raise ValueError(f"路径越界: {path}")
    return p

def do_tool(cwd, name, args):
    try:
        if name == "read_file":
            p = safe(cwd, args["path"])
            lines = open(p, encoding="utf-8").read().splitlines()
            a, b = args.get("start", 1), args.get("end", len(lines))
            body = "\n".join(f"{i}\t{l}" for i, l in enumerate(lines[a-1:b], a))
            return body[:OUT_CAP]
        if name == "write_file":
            p = safe(cwd, args["path"]); os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "w", encoding="utf-8").write(args["content"]); return f"已写 {args['path']} ({len(args['content'])} 字节)"
        if name == "edit_file":
            p = safe(cwd, args["path"]); txt = open(p, encoding="utf-8").read()
            if txt.count(args["old"]) != 1:
                return f"失败: old 在文件中出现 {txt.count(args['old'])} 次(需恰好 1 次)"
            open(p, "w", encoding="utf-8").write(txt.replace(args["old"], args["new"])); return f"已改 {args['path']}"
        if name == "delete_file":
            p = safe(cwd, args["path"]); os.remove(p); return f"已删 {args['path']}"
        if name == "list_dir":
            p = safe(cwd, args["path"]); return "\n".join(sorted(os.listdir(p)))[:OUT_CAP]
        if name == "grep":
            r = subprocess.run(["grep", "-rn", "--exclude-dir=node_modules", "--exclude-dir=target", args["pattern"], safe(cwd, args["path"])], capture_output=True, text=True)
            return (r.stdout or "(无匹配)")[:OUT_CAP]
        if name == "run":
            r = subprocess.run(args["cmd"], shell=True, cwd=cwd, capture_output=True, text=True, timeout=420)
            return f"exit={r.returncode}\n{(r.stdout + r.stderr)[-OUT_CAP:]}"
        return f"未知工具 {name}"
    except Exception as e:
        return f"工具错误: {e}"

def call_api(key, model, messages):
    req = urllib.request.Request(API_URL, method="POST",
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
        data=json.dumps({"model": model, "messages": messages, "tools": tools_spec(), "max_tokens": 8000, "temperature": 0}).encode())
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)["choices"][0]["message"]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--task", required=True); ap.add_argument("--cwd", required=True)
    ap.add_argument("--report", required=True); ap.add_argument("--max-steps", type=int, default=40)
    ap.add_argument("--model", default="deepseek-v4-pro")
    a = ap.parse_args()
    key = os.environ.get("DEEPSEEK_API_KEY")
    if not key: sys.exit("缺 DEEPSEEK_API_KEY")
    cwd = os.path.realpath(a.cwd)
    task = open(a.task, encoding="utf-8").read()
    log = open(a.report + ".log", "w", encoding="utf-8")
    def L(s): log.write(s + "\n"); log.flush(); print(s, flush=True)

    messages = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": task}]
    for step in range(1, a.max_steps + 1):
        try:
            msg = call_api(key, a.model, messages)
        except urllib.error.HTTPError as e:
            L(f"[HTTP {e.code}] {e.read().decode()[:500]}"); break
        rc = msg.get("reasoning_content")
        if rc: L(f"--- step {step} reasoning ---\n{rc[:800]}")
        tcs = msg.get("tool_calls") or []
        # 回灌 assistant 轮(带 tool_calls)
        messages.append({"role": "assistant", "content": msg.get("content") or "", "tool_calls": tcs})
        if not tcs:
            if msg.get("content"): L(f"[step {step} 裸文本] {msg['content'][:400]}")
            messages.append({"role": "user", "content": "请用工具继续,或调用 finish 结束。"})
            continue
        for tc in tcs:
            name = tc["function"]["name"]
            try: args = json.loads(tc["function"]["arguments"] or "{}")
            except Exception: args = {}
            if name == "finish":
                rep = args.get("report", "(空报告)")
                open(a.report, "w", encoding="utf-8").write(rep)
                L(f"=== FINISH (step {step}) ===\n{rep}"); log.close(); return
            preview = json.dumps(args, ensure_ascii=False)
            L(f"[step {step}] {name} {preview[:300]}")
            result = do_tool(cwd, name, args)
            L(f"   -> {result[:400]}")
            messages.append({"role": "tool", "tool_call_id": tc["id"], "content": result})
    L("=== 达到 max-steps 未 finish ==="); log.close()

if __name__ == "__main__":
    main()
