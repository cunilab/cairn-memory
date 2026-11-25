# CAIRN_SETUP_MANAGED
"""Hermes's native plugin entry point. Capture and retrieval live in the Cairn CLI."""

import json
import subprocess

EXE, CONFIG = __CAIRN_PATHS__


def register(ctx):
    def forward(event, session_id, **fields):
        try:
            cwd = ctx.session_cwd()
            if not session_id or not cwd:
                return None
            result = subprocess.run(
                [EXE, "hook", event, "--agent", "hermes", "--config", CONFIG, "--cairn-managed"],
                cwd=cwd,
                input=json.dumps({"session_id": session_id, "cwd": cwd, **fields}),
                encoding="utf-8",
                stdout=subprocess.PIPE,
                timeout=1 if event == "PreToolUse" else 6,
                check=True,
            )
            if result.stdout:
                return json.loads(result.stdout)
        except (OSError, TypeError, ValueError, subprocess.SubprocessError):
            # Cairn capture is best effort, including in a before-tool observer.
            return None

    def on_prompt(session_id, user_message, is_first_turn=False, **_):
        # Hermes ignores on_session_start output. Its first prompt can carry the brief.
        reply = forward("SessionStart", session_id) if is_first_turn else None
        forward("UserPromptSubmit", session_id, user_message=user_message)
        if reply:
            text = reply.get("hookSpecificOutput", {}).get("additionalContext")
            if text:
                return {"context": text}

    def relevant(tool_name):
        return tool_name in {"terminal", "read_file", "write_file", "patch"} or tool_name.startswith(
            ("mcp__cairn__", "mcp_cairn_")
        )

    def before_tool(tool_name, args, session_id=None, tool_call_id=None, **_):
        if relevant(tool_name):
            forward("PreToolUse", session_id, tool_name=tool_name, args=args, tool_call_id=tool_call_id)

    def after_tool(tool_name, args, result, session_id=None, tool_call_id=None, status=None, error_message=None, **_):
        if relevant(tool_name):
            forward("PostToolUse", session_id, tool_name=tool_name, args=args,
                    result=result, status=status, error_message=error_message, tool_call_id=tool_call_id)

    def after_turn(session_id, assistant_response, **_):
        forward("Stop", session_id, assistant_response=assistant_response)

    def failed_turn(session_id, failed=False, interrupted=False, **_):
        if failed or interrupted:
            forward("Stop", session_id)

    def after_compaction(session_id, old_session_id=None, **_):
        forward("PreCompact", old_session_id or session_id)

    def end_session(session_id=None, **_):
        forward("SessionEnd", session_id)

    ctx.register_hook("pre_llm_call", on_prompt)
    ctx.register_hook("pre_tool_call", before_tool)
    ctx.register_hook("post_tool_call", after_tool)
    ctx.register_hook("post_llm_call", after_turn)
    ctx.register_hook("on_session_end", failed_turn)
    ctx.register_hook("post_compaction", after_compaction)
    ctx.register_hook("on_session_finalize", end_session)
