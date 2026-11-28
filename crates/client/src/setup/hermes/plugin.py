# CAIRN_SETUP_MANAGED
"""Hermes's native plugin entry point and memory provider. Capture and retrieval live in the Cairn CLI."""

import hmac
import json
import os
import re
import subprocess
import sys
import threading
import time
import types
import uuid
from functools import wraps

try:
    from agent.memory_provider import MemoryProvider
except ImportError:  # A host without memory providers loads the tool hooks only.
    MemoryProvider = object

EXE, CONFIG = __CAIRN_PATHS__


GATEWAY = __CAIRN_GATEWAY__
TOOLS = json.loads(__CAIRN_TOOLS__)
SKILL_TOOLS = [tool for tool in TOOLS if tool["name"].startswith("skill_")]
TOOL_NAMES = {tool["name"] for tool in TOOLS}
IDLE_SECONDS = 30 * 60
BACKGROUND = ("cron", "subagent", "flush")
UNAVAILABLE = "Cairn memory is unavailable in this Hermes turn."
# Hermes builds the provider per session from disk, but binds chat memory only when the process loads plugins.
UNBOUND = ("Cairn chat memory is not loaded in this gateway process. Start the Hermes gateway process again "
           "after cairn setup hermes --gateway, then send /new.")


def background(session_id=None, task_id=None, platform=None, agent_context=None, **_):
    if platform in BACKGROUND or agent_context in BACKGROUND:
        return True
    if isinstance(session_id, str) and session_id.startswith(("cron_", "flush_")):
        return True
    if isinstance(task_id, str) and task_id.startswith("cron:"):
        return True
    try:
        from agent.delegation_context import is_delegated_child_process_context
        if is_delegated_child_process_context():
            return True
    except ImportError:
        pass
    try:
        from gateway.session_context import bound_session_env
        return bound_session_env("HERMES_CRON_SESSION") == "1"
    except ImportError:
        return False


def turn_cwd():
    """The running turn's session cwd: a gateway process binds it per turn, and its own cwd is the Hermes
    install. Empty outside a turn and on older Hermes."""
    try:
        from agent.runtime_cwd import scoped_session_cwd
    except ImportError:
        return ""
    return scoped_session_cwd()


def foreground(callback):
    @wraps(callback)
    def call(*args, **kwargs):
        if not background(**kwargs):
            return callback(*args, **kwargs)
    return call


def forward(event, session_id, cwd=None, **fields):
    try:
        if not session_id or (not cwd and not GATEWAY):
            return None
        result = subprocess.run(
            [EXE, "hook", event, "--agent", "hermes", "--config", CONFIG]
            + ([] if GATEWAY else ["--cairn-managed"]),
            cwd=cwd,
            input=json.dumps({"session_id": session_id, "cwd": cwd, **fields}),
            encoding="utf-8",
            stdout=subprocess.PIPE,
            timeout={"PostToolUse": 7, "Tool": 65}.get(event, 6),
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


def attach(result, text):
    try:
        data = json.loads(result)
        if isinstance(data, dict):
            data["cairn_recall"] = text
            return json.dumps(data)
    except (TypeError, ValueError):
        pass
    return str(result) + "\n" + text


def home_key(home=None):
    try:
        from hermes_constants import hermes_home_key
        return hermes_home_key(home)
    except ImportError:
        return str(home or "")


def chat_profiles():
    # Hermes imports this file twice (plugin and memory provider) under different module names, and one
    # gateway process can serve several profiles: chat sessions live in one table keyed by profile home.
    shared = sys.modules.setdefault("_cairn_memory_hermes", types.ModuleType("_cairn_memory_hermes"))
    return shared.__dict__.setdefault("profiles", {})


def chat_digest(salt, platform, field, raw):
    parts = json.dumps([platform, field, raw], separators=(",", ":"))
    return hmac.digest(salt.encode(), parts.encode(), "sha256").hex()


CHAT_TOOLS = [
    ("memory_search", "ChatSearch",
     "Search this chat's memories. Pass a project ID or exact name when you also need memories from that project.",
     {"query": {"type": "string"}}, ["query"]),
    ("memory_save", "ChatSave",
     "Save lasting knowledge for this chat. Pass a project ID or exact name only to share a note, rule, decision, gotcha, procedure, or fact with that project. Save preferences without a project.",
     {"kind": {"type": "string", "enum": ["note", "decision", "rule", "preference",
                                         "procedure", "gotcha", "fact"]},
      "title": {"type": "string", "maxLength": 200, "description": "About 80 characters."},
      "summary": {"type": "string", "maxLength": 1000, "description": "About 240 characters."},
      "reason": {"type": "string"},
      "fields": {"type": "object"}}, ["kind", "title", "summary"]),
]
for _name, _event, _description, _properties, _required in CHAT_TOOLS:
    _properties["project"] = {"type": "string", "description": "Project ID or exact name."}
CHAT_SCHEMAS = [{"name": name, "description": description,
                 "parameters": {"type": "object", "properties": properties, "required": required}}
                for name, _, description, properties, required in CHAT_TOOLS]
CHAT_SCHEMAS += [{"name": skill["name"], "description": skill["description"], "parameters": skill["inputSchema"]}
                 for skill in SKILL_TOOLS]
CLI_SCHEMAS = [{"name": tool["name"], "description": tool["description"], "parameters": tool["inputSchema"]}
               for tool in TOOLS]


class Chat:
    """One gateway profile's Cairn chat sessions: hashed routes and 30-minute idle sessions."""

    def __init__(self, salt, bound_session_env):
        self.salt = salt
        self.bound = bound_session_env
        self.sessions = {}
        # Hermes session id -> chat state: teardown (/new, /reset, shutdown) runs without a bound route.
        self.hosts = {}
        self.lock = threading.Lock()
        # Set once transform_tool_result is registered: it delivers the terminal note.
        self.transform = False

    def route(self, session_id=None, **details):
        if background(session_id=session_id, **details):
            return None

        def value(field):
            return self.bound("HERMES_SESSION_" + field)

        platform, chat_id, kind = value("PLATFORM"), value("CHAT_ID"), value("CHAT_TYPE")
        if not re.fullmatch(r"[a-z][a-z0-9_-]{0,31}", platform) or not chat_id or not kind:
            return None
        if platform in BACKGROUND:
            return None
        bound_session = value("ID")
        if session_id and bound_session and session_id != bound_session:
            return None
        # Discord reports the thread as its effective channel, with its parent separately.
        parent = value("PARENT_CHAT_ID")
        thread = value("THREAD_ID")
        dm = kind in {"dm", "direct", "private"}
        chat_hash = chat_digest(self.salt, platform, "chat", parent or chat_id)
        thread_hash = chat_digest(self.salt, platform, "thread", thread) if thread else None
        chat = {"platform": platform, "kind": "dm" if dm else "group",
                "chat_id_hash": chat_hash, "thread_id_hash": thread_hash}
        return (chat_hash, thread_hash), chat

    def send(self, event, state, chat, **fields):
        return forward(event, state["id"], chat_context=chat, **fields)

    def close(self, state):
        self.send("Stop", state, state["chat"])
        self.send("SessionEnd", state, state["chat"])

    def drop(self, state):
        """Forgets a chat state (caller holds the lock); False when it was already gone."""
        if self.sessions.get(state["key"]) is not state:
            return False
        del self.sessions[state["key"]]
        if state["timer"] is not None:
            state["timer"].cancel()
        for host in state["hosts"]:
            if self.hosts.get(host) is state:
                del self.hosts[host]
        return True

    def idle(self, key, state, timer):
        with self.lock:
            if self.sessions.get(key) is not state or state["timer"] is not timer:
                return
            self.drop(state)
        self.close(state)

    def end(self, session_id):
        """Hermes finalized this host session (/new, /reset or shutdown): its chat's Cairn session ends now,
        or when the running turn ends."""
        with self.lock:
            state = self.hosts.get(session_id)
            if state is None:
                return
            state["ended"] = True
            if state["turns"] or not self.drop(state):
                return
        self.close(state)

    def finalize(self, session_id=None, **_):
        if isinstance(session_id, str) and session_id:
            self.end(session_id)

    def prompt(self, session_id, user_message, **details):
        """Returns (chat instruction for a new chat session, prompt recall)."""
        current = self.route(session_id, **details)
        if current is None:
            return None, None
        key, chat = current
        expired = None
        with self.lock:
            old = self.sessions.get(key)
            # An ended session whose turn never reported its end closes here at the latest.
            if old is not None and (old["ended"] or (
                    old["timer"] is not None and time.monotonic() >= old["deadline"])):
                self.drop(old)
                expired = old
            first = key not in self.sessions
            if first:
                self.sessions[key] = {"id": "hchat_" + uuid.uuid4().hex, "key": key, "hosts": set(),
                                      "ended": False, "turns": set(), "timer": None}
            state = self.sessions[key]
            if session_id:
                state["hosts"].add(session_id)
                self.hosts[session_id] = state
            if state["timer"] is not None:
                state["timer"].cancel()
                state["timer"] = None
            state["turns"].add(session_id)
            state["chat"] = chat
        if expired is not None:
            self.close(expired)
        start = context(self.send("SessionStart", state, chat)) if first else None
        recall = None
        if isinstance(user_message, str):
            recall = context(self.send("UserPromptSubmit", state, chat, user_message=user_message))
        return start, recall

    def turn_end(self, session_id, **details):
        current = self.route(session_id, **details)
        if current is None:
            return
        key, _ = current
        with self.lock:
            state = self.sessions.get(key)
            if state is None or session_id not in state["turns"]:
                return
            state["turns"].remove(session_id)
            if state["turns"]:
                return
            ended = state["ended"] and self.drop(state)
            if not ended:
                timer = threading.Timer(IDLE_SECONDS, lambda: self.idle(key, state, timer))
                state["timer"] = timer
                state["deadline"] = time.monotonic() + IDLE_SECONDS
                timer.daemon = True
                timer.start()
        if ended:
            self.close(state)

    def active(self, **details):
        current = self.route(**details)
        if current is None:
            return None, None
        key, chat = current
        with self.lock:
            return self.sessions.get(key), chat

    def post(self, tool_name, args, result, tool_call_id=None, status=None, error_message=None, **details):
        if tool_name != "terminal":
            return None
        state, chat = self.active(**details)
        if state is not None:
            return context(self.send("PostToolUse", state, chat, tool_name=tool_name, args=args, result=result,
                                     tool_call_id=tool_call_id, status=status, error_message=error_message))

    def after_tool(self, tool_name, **details):
        # A terminal call goes through the transform, whose result reaches the model.
        if not self.transform:
            self.post(tool_name, **details)

    def append_recall(self, tool_name, result, **details):
        if text := self.post(tool_name, result=result, **details):
            return attach(result, text)

    def tool(self, name, args, session_id):
        state, chat = self.active(session_id=session_id)
        reply = None
        if state is not None and isinstance(args, dict):
            for tool, event, _, properties, _ in CHAT_TOOLS:
                if tool == name:
                    reply = self.send(event, state, chat,
                                      arguments={key: args[key] for key in properties if key in args})
            for skill in SKILL_TOOLS:
                if skill["name"] == name:
                    properties = skill["inputSchema"]["properties"]
                    reply = self.send("ChatSkill", state, chat, skill_tool=name,
                                      arguments={key: args[key] for key in properties if key in args})
        return json.dumps(reply or {"error": "Cairn chat memory is unavailable."})


class CairnProvider(MemoryProvider):
    """Hermes's memory provider: brief, prompt recall, checkpoints, session boundaries and Cairn tools.
    Tool events have no provider method, so they stay plugin hooks."""

    name = "cairn-memory"

    def __init__(self):
        self.session_id = ""
        self.cwd = None
        self.host_cwd = None
        self.primary = True
        self.chat = None
        self.started = set()
        self.brief = None
        self.recall = None

    def is_available(self):
        return True

    def initialize(self, session_id, **kwargs):
        self.session_id = session_id
        self.host_cwd = kwargs.get("cwd")
        self.cwd = self.host_cwd or turn_cwd() or os.getcwd()
        # Older Hermes omits agent_context; the background heuristics still apply on every call.
        self.primary = (kwargs.get("agent_context") or "primary") == "primary" and not background(
            session_id=session_id, platform=kwargs.get("platform"))
        if GATEWAY:
            # Chat identity kwargs (raw chat and user ids) are ignored: routes come from the bound turn.
            self.chat = chat_profiles().get(home_key(kwargs.get("hermes_home")))

    def live(self):
        return self.primary and not background(session_id=self.session_id)

    def get_tool_schemas(self):
        # Hermes reads schemas once before initialize (routing) and again after it (what the model sees).
        if self.session_id and (not self.live() or (GATEWAY and self.chat is None)):
            return []
        return CHAT_SCHEMAS if GATEWAY else CLI_SCHEMAS

    def handle_tool_call(self, tool_name, args, **kwargs):
        if not self.live():
            return json.dumps({"error": UNAVAILABLE})
        if GATEWAY:
            return self.chat.tool(tool_name, args, self.session_id) if self.chat else json.dumps(
                {"error": UNBOUND})
        if tool_name not in TOOL_NAMES or not isinstance(args, dict):
            return json.dumps({"error": "unknown Cairn tool"})
        reply = forward("Tool", self.session_id, self.cwd, tool_name=tool_name, arguments=args)
        return json.dumps(reply if reply is not None else {"error": UNAVAILABLE})

    def on_turn_start(self, turn_number, message, **kwargs):
        # Every turn reaches here; prefetch skips trivial prompts, so it only delivers what this stored.
        if not self.live():
            return
        # Kept for the turn's later calls, which may run outside its scope.
        self.cwd = self.host_cwd or turn_cwd() or self.cwd
        if GATEWAY:
            if self.chat is not None:
                start, self.recall = self.chat.prompt(self.session_id, message)
                self.brief = start or self.brief
            return
        if self.session_id not in self.started:
            self.started.add(self.session_id)
            self.brief = context(forward("SessionStart", self.session_id, self.cwd)) or self.brief
        self.recall = context(forward("UserPromptSubmit", self.session_id, self.cwd, user_message=message))

    def prefetch(self, query, *, session_id=""):
        notes = [text for text in (self.brief, self.recall) if text]
        self.brief = self.recall = None
        return "\n".join(notes)

    def sync_turn(self, user_content, assistant_content, *, session_id="", **_):
        if not GATEWAY and self.live():
            forward("Stop", session_id or self.session_id, self.cwd, assistant_response=assistant_content)

    def on_pre_compress(self, messages, **_):
        if not GATEWAY and self.live():
            forward("PreCompact", self.session_id, self.cwd)
        return ""

    def on_session_end(self, messages):
        if not GATEWAY and self.live():
            forward("SessionEnd", self.session_id, self.cwd)

    def on_session_switch(self, new_session_id, *, parent_session_id="", reset=False, rewound=False, **kwargs):
        if rewound or not new_session_id:
            return
        if GATEWAY:
            if reset and self.chat is not None:
                self.chat.end(self.session_id)
        elif kwargs.get("reason") == "compression":
            self.started.add(new_session_id)  # The same conversation continues: no second brief.
        self.session_id = new_session_id


def register(ctx):
    if hasattr(ctx, "register_memory_provider"):
        ctx.register_memory_provider(CairnProvider())
    if not hasattr(ctx, "get_config"):
        return  # Hermes's memory loader: the plugin load registers the tool hooks.
    if GATEWAY:
        return register_gateway(ctx)
    transform = False

    def post(tool_name, args, result, session_id=None, tool_call_id=None, status=None, error_message=None, **_):
        # Cairn's own tools are read as the MCP tools they mirror (saves and lookups).
        name = "mcp_cairn_" + tool_name if tool_name in TOOL_NAMES else tool_name
        return context(forward("PostToolUse", session_id, ctx.session_cwd(), tool_name=name, args=args,
                               result=result, status=status, error_message=error_message,
                               tool_call_id=tool_call_id))

    @foreground
    def after_tool(tool_name, **details):
        # A terminal call goes through the transform, whose result reaches the model.
        if tool_name == "terminal" and transform:
            return
        if tool_name in {"terminal", "read_file", "write_file", "patch"} or tool_name in TOOL_NAMES:
            post(tool_name, **details)

    @foreground
    def append_recall(tool_name, result, **details):
        if tool_name == "terminal" and (text := post(tool_name, result=result, **details)):
            return attach(result, text)

    @foreground
    def turn_end(session_id=None, failed=False, interrupted=False, **_):
        # The provider's sync_turn checkpoints completed turns; Hermes skips it for these.
        if failed or interrupted:
            forward("Stop", session_id, ctx.session_cwd())

    ctx.register_hook("post_tool_call", after_tool)
    try:
        ctx.register_hook("transform_tool_result", append_recall)
        transform = True
    except ValueError:  # Older hosts have no result transform: the note cannot reach the model.
        pass
    ctx.register_hook("on_session_end", turn_end)


def register_gateway(ctx):
    try:
        from gateway.session_context import bound_session_env
    except ImportError:
        return  # A host without trusted per-turn context cannot use chat memory.

    salt = ctx.get_config("chat_salt")
    if not isinstance(salt, str) or not salt:
        return
    chat = Chat(salt, bound_session_env)
    chat_profiles()[home_key()] = chat
    ctx.register_hook("post_tool_call", chat.after_tool)
    try:
        ctx.register_hook("transform_tool_result", chat.append_recall)
        chat.transform = True
    except ValueError:
        pass
    ctx.register_hook("on_session_end", chat.turn_end)
    try:
        # /new, /reset and shutdown tear the agent down and finalize its session; agent eviction does not.
        ctx.register_hook("on_session_finalize", chat.finalize)
    except ValueError:
        pass
