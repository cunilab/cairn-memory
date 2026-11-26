# CAIRN_SETUP_MANAGED
"""Hermes's native plugin entry point. Capture and retrieval live in the Cairn CLI."""

import hmac
import json
import subprocess
import threading
import time
import uuid

EXE, CONFIG = __CAIRN_PATHS__


GATEWAY = __CAIRN_GATEWAY__
IDLE_SECONDS = 30 * 60


def forward(ctx, event, session_id, **fields):
    try:
        cwd = ctx.session_cwd() if not GATEWAY else None
        if not session_id or (not cwd and not GATEWAY):
            return None
        result = subprocess.run(
            [EXE, "hook", event, "--agent", "hermes", "--config", CONFIG]
            + ([] if GATEWAY else ["--cairn-managed"]),
            cwd=cwd,
            input=json.dumps({"session_id": session_id, "cwd": cwd, **fields}),
            encoding="utf-8",
            stdout=subprocess.PIPE,
            timeout=7 if event == "PostToolUse" else 6,
            check=True,
        )
        if result.stdout:
            return json.loads(result.stdout)
    except (OSError, TypeError, ValueError, subprocess.SubprocessError):
        # Cairn capture is best effort.
        return None


def context(reply):
    if isinstance(reply, dict):
        return reply.get("hookSpecificOutput", {}).get("additionalContext")


def register(ctx):
    if GATEWAY:
        return register_gateway(ctx)
    pending = {}

    def on_prompt(session_id, user_message, is_first_turn=False, **_):
        # Hermes ignores on_session_start output. Its first prompt can carry the brief.
        reply = forward(ctx, "SessionStart", session_id) if is_first_turn else None
        recall = forward(ctx, "UserPromptSubmit", session_id, user_message=user_message)
        notes = list(pending.pop(session_id, {}).values())
        if text := context(reply):
            notes.insert(0, text)
        if text := context(recall):
            notes.append(text)
        if notes:
            return {"context": "\n".join(notes)}

    def relevant(tool_name):
        return tool_name in {"terminal", "read_file", "write_file", "patch"} or tool_name.startswith(
            ("mcp__cairn__", "mcp_cairn_")
        )

    def after_tool(tool_name, args, result, session_id=None, tool_call_id=None, status=None, error_message=None, **_):
        if relevant(tool_name):
            reply = forward(ctx, "PostToolUse", session_id, tool_name=tool_name, args=args,
                            result=result, status=status, error_message=error_message, tool_call_id=tool_call_id)
            if text := context(reply):
                pending.setdefault(session_id, {})[tool_call_id] = text

    def append_recall(tool_name, result, session_id=None, tool_call_id=None, **_):
        if tool_name != "terminal":
            return None
        notes = pending.get(session_id)
        text = notes.pop(tool_call_id, None) if notes else None
        if text:
            try:
                data = json.loads(result)
                if isinstance(data, dict):
                    data["cairn_recall"] = text
                    return json.dumps(data)
            except (TypeError, ValueError):
                pass
            return result + "\n" + text

    def after_turn(session_id, assistant_response, **_):
        forward(ctx, "Stop", session_id, assistant_response=assistant_response)

    def failed_turn(session_id, failed=False, interrupted=False, **_):
        if failed or interrupted:
            forward(ctx, "Stop", session_id)

    def after_compaction(session_id, old_session_id=None, **_):
        forward(ctx, "PreCompact", old_session_id or session_id)

    def end_session(session_id=None, **_):
        pending.pop(session_id, None)
        forward(ctx, "SessionEnd", session_id)

    ctx.register_hook("pre_llm_call", on_prompt)
    ctx.register_hook("post_tool_call", after_tool)
    try:
        ctx.register_hook("transform_tool_result", append_recall)
    except ValueError:  # Older hosts deliver pending notes on the next user turn.
        pass
    ctx.register_hook("post_llm_call", after_turn)
    ctx.register_hook("on_session_end", failed_turn)
    ctx.register_hook("post_compaction", after_compaction)
    ctx.register_hook("on_session_finalize", end_session)


def chat_digest(salt, platform, field, raw):
    parts = json.dumps([platform, field, raw], separators=(",", ":"))
    return hmac.digest(salt.encode(), parts.encode(), "sha256").hex()


def register_gateway(ctx):
    try:
        from gateway.session_context import bound_session_env
        from tools.registry import no_cache_check_fn
    except ImportError:
        return  # A host without trusted per-turn context cannot use chat memory.

    salt = ctx.get_config("chat_salt")
    if not isinstance(salt, str) or not salt:
        return
    owners = ctx.get_config("chat_owner_hashes", {})
    projects = ctx.get_config("chat_projects", {})
    owners = owners if isinstance(owners, dict) else {}
    projects = projects if isinstance(projects, dict) else {}
    sessions = {}
    lock = threading.Lock()

    def route():
        def value(field):
            return bound_session_env("HERMES_SESSION_" + field)

        platform, chat_id, kind = value("PLATFORM"), value("CHAT_ID"), value("CHAT_TYPE")
        if platform not in {"telegram", "discord"} or not chat_id or not kind:
            return None
        # Discord reports the thread as its effective channel, with its parent separately.
        parent = value("PARENT_CHAT_ID")
        thread = value("THREAD_ID")
        sender = value("ACTOR_ID")
        dm = kind in {"dm", "direct", "private"}
        chat_hash = chat_digest(salt, platform, "chat", parent or chat_id)
        sender_hash = chat_digest(salt, platform, "sender", sender) if sender else None
        thread_hash = chat_digest(salt, platform, "thread", thread) if thread else None
        owner_hash = owners.get(platform)
        owner = bool(dm and sender_hash and isinstance(owner_hash, str)
                     and owner_hash.isascii() and hmac.compare_digest(sender_hash, owner_hash))
        chat = {"platform": platform, "kind": "dm" if dm else "group",
                "chat_id_hash": chat_hash, "sender_id_hash": sender_hash,
                "thread_id_hash": thread_hash, "owner": owner}
        return (chat_hash, thread_hash), chat

    def send(event, state, chat, **fields):
        return forward(ctx, event, state["id"], chat_context=chat,
                       project_id=projects.get(chat["chat_id_hash"]), **fields)

    def idle(key, state, timer, chat):
        with lock:
            if sessions.get(key) is not state or state["timer"] is not timer:
                return
            del sessions[key]
        send("Stop", state, chat)
        send("SessionEnd", state, chat)

    def on_prompt(session_id, user_message, turn_id=None, **_):
        current = route()
        if current is None:
            return
        key, chat = current
        expired = None
        with lock:
            old = sessions.get(key)
            if old is not None and old["timer"] is not None and time.monotonic() >= old["deadline"]:
                old["timer"].cancel()
                expired = sessions.pop(key)
            first = key not in sessions
            if first:
                sessions[key] = {"id": "hchat_" + uuid.uuid4().hex,
                                 "turns": set(), "timer": None, "recall": {}}
            state = sessions[key]
            if state["timer"] is not None:
                state["timer"].cancel()
                state["timer"] = None
            state["turns"].add(turn_id or session_id)
        if expired is not None:
            send("Stop", expired, expired["last_chat"])
            send("SessionEnd", expired, expired["last_chat"])
        notes = []
        if first and (text := context(send("SessionStart", state, chat))):
            notes.append(text)
        if isinstance(user_message, str):
            if text := context(send("UserPromptSubmit", state, chat, user_message=user_message)):
                notes.append(text)
        if notes:
            return {"context": "\n".join(notes)}

    def after_turn(session_id, turn_id=None, **_):
        current = route()
        if current is None:
            return
        key, chat = current
        with lock:
            state = sessions.get(key)
            turn = turn_id or session_id
            if state is None or turn not in state["turns"]:
                return
            state["turns"].remove(turn)
            if state["turns"]:
                return
            state["recall"].clear()
            timer = threading.Timer(IDLE_SECONDS, lambda: idle(key, state, timer, chat))
            state["timer"] = timer
            state["deadline"] = time.monotonic() + IDLE_SECONDS
            state["last_chat"] = chat
            timer.daemon = True
            timer.start()

    def active():
        current = route()
        if current is None:
            return None, None
        key, chat = current
        with lock:
            return sessions.get(key), chat

    def after_tool(tool_name, args, result, tool_call_id=None, status=None, error_message=None, **_):
        if tool_name != "terminal":
            return
        state, chat = active()
        if state is not None:
            reply = send("PostToolUse", state, chat, tool_name=tool_name, args=args, result=result,
                         tool_call_id=tool_call_id, status=status, error_message=error_message)
            if tool_call_id and (text := context(reply)):
                with lock:
                    state["recall"][tool_call_id] = text

    def append_recall(tool_name, result, tool_call_id=None, **_):
        if tool_name != "terminal":
            return
        state, _ = active()
        if state is None:
            return
        with lock:
            text = state["recall"].pop(tool_call_id, None)
        if text:
            try:
                data = json.loads(result)
                if isinstance(data, dict):
                    data["cairn_recall"] = text
                    return json.dumps(data)
            except (TypeError, ValueError):
                pass
            return str(result) + "\n" + text

    def tool(event):
        def call(args, **_):
            state, chat = active()
            reply = send(event, state, chat, arguments=args) if state is not None else None
            return json.dumps(reply or {"error": "Cairn chat memory is unavailable."})
        return call

    available = no_cache_check_fn(lambda: route() is not None)
    for name, event, description, properties, required in [
        ("memory_search", "ChatSearch", "Search memories available in this chat.",
         {"query": {"type": "string"}}, ["query"]),
        ("memory_save", "ChatSave", "Save a personal memory tagged with this chat.",
         {"kind": {"type": "string", "enum": ["note", "decision", "rule", "preference",
                                             "procedure", "gotcha", "fact"]},
          "title": {"type": "string", "maxLength": 80},
          "summary": {"type": "string", "maxLength": 240},
          "reason": {"type": "string"},
          "fields": {"type": "object"}}, ["kind", "title", "summary"]),
    ]:
        schema = {"name": name, "description": description,
                  "parameters": {"type": "object", "properties": properties, "required": required}}
        ctx.register_tool(name=name, toolset="cairn-chat", schema=schema, handler=tool(event),
                          check_fn=available, description=description)

    ctx.register_hook("pre_llm_call", on_prompt)
    ctx.register_hook("post_tool_call", after_tool)
    try:
        ctx.register_hook("transform_tool_result", append_recall)
    except ValueError:
        pass
    ctx.register_hook("on_session_end", after_turn)
