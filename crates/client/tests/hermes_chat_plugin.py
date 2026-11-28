"""The Hermes plugin and memory provider against stubbed Hermes callbacks (no Hermes install needed)."""

import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

PLUGIN = Path(__file__).resolve().parents[1] / "src/setup/hermes/plugin.py"
TOOLS = json.loads((Path(__file__).parent / "fixtures/tools_list.json").read_text())["tools"]
SKILL_TOOLS = [tool for tool in TOOLS if tool["name"].startswith("skill_")]


class Context:
    """Hermes's PluginContext for a plugin loaded through plugins.enabled."""

    def __init__(self, settings):
        self.settings = settings
        self.hooks = {}
        self.tools = {}
        self.providers = []

    def get_config(self, key, default=None):
        return self.settings.get(key, default)

    def register_hook(self, name, callback):
        self.hooks[name] = callback

    def register_tool(self, **tool):
        self.tools[tool["name"]] = tool

    def register_memory_provider(self, provider):
        self.providers.append(provider)

    def session_cwd(self):
        return "/stubbed-cwd"


class Collector:
    """Hermes's memory loader context: only register_* calls exist on it."""

    def __init__(self):
        self.provider = None
        self.hooks = {}

    def register_memory_provider(self, provider):
        self.provider = provider

    def __getattr__(self, name):
        if not name.startswith("register_"):
            raise AttributeError(name)
        return lambda *args, **kwargs: self.hooks.setdefault(name, []).append(args)


class Timer:
    def __init__(self, delay, callback):
        self.delay = delay
        self.callback = callback
        self.cancelled = False

    def start(self):
        pass

    def cancel(self):
        self.cancelled = True


class MemoryProvider:
    pass


class Hermes(unittest.TestCase):
    gateway = False

    def setUp(self):
        self.bound = {}
        self.requests = []
        self.timers = []
        self.delegated = False
        home = tempfile.TemporaryDirectory(prefix="cairn-hermes-test-")
        self.addCleanup(home.cleanup)
        self.home = home.name
        env = patch.dict("os.environ", {"HERMES_HOME": home.name})
        env.start()
        self.addCleanup(env.stop)
        gateway = types.ModuleType("gateway.session_context")
        gateway.bound_session_env = lambda key: self.bound.get(key.removeprefix("HERMES_SESSION_"), "")
        delegation = types.ModuleType("agent.delegation_context")
        delegation.is_delegated_child_process_context = lambda: self.delegated
        provider = types.ModuleType("agent.memory_provider")
        provider.MemoryProvider = MemoryProvider
        constants = types.ModuleType("hermes_constants")
        constants.hermes_home_key = lambda path=None: str(Path(path or self.home).resolve())
        modules = patch.dict(sys.modules, {
            "gateway.session_context": gateway, "agent.delegation_context": delegation,
            "agent.memory_provider": provider, "hermes_constants": constants,
        })
        modules.start()
        self.addCleanup(modules.stop)
        sys.modules.pop("_cairn_memory_hermes", None)
        # Hermes imports the folder twice: as a plugin and as the memory provider.
        self.plugin = self.load_plugin()
        self.provider_module = self.load_plugin()
        for target, replacement in [("subprocess.run", self.run_hook), ("threading.Timer", self.timer)]:
            module, name = target.split(".")
            stub = patch.object(self.plugin[module], name, replacement)
            stub.start()
            self.addCleanup(stub.stop)
        self.salt = "test-only-salt"
        self.ctx = Context({"chat_salt": self.salt})
        self.plugin["register"](self.ctx)

    def load_plugin(self):
        namespace = {}
        script = PLUGIN.read_text().replace("__CAIRN_PATHS__", '["cairn", "selected.json"]')
        script = script.replace("__CAIRN_GATEWAY__", str(self.gateway)).replace(
            "__CAIRN_TOOLS__", json.dumps(json.dumps(TOOLS))
        )
        exec(compile(script, str(PLUGIN), "exec"), namespace)
        return namespace

    def provider(self, session_id="host-session", **kwargs):
        collector = Collector()
        self.provider_module["register"](collector)
        self.assertEqual(collector.hooks, {}, "the memory loader registers no hooks")
        provider = collector.provider
        self.assertTrue(provider.is_available())
        details = {"platform": "telegram" if self.gateway else "cli", "hermes_home": self.home,
                   "agent_context": "primary", "cwd": "/repo", "user_id": "user-owner",
                   "chat_id": "chat-transport-id", **kwargs}
        provider.initialize(session_id, **{key: value for key, value in details.items() if value is not None})
        return provider

    def run_hook(self, command, input, cwd=None, **_):
        request = json.loads(input)
        event = command[2]
        self.requests.append((event, request))
        notes = {"SessionStart": "custom instruction", "UserPromptSubmit": "recall"}
        if event in notes:
            return types.SimpleNamespace(stdout=json.dumps({"hookSpecificOutput": {"additionalContext": notes[event]}}))
        if event == "PostToolUse" and request["tool_name"] == "terminal":
            return types.SimpleNamespace(stdout=json.dumps({"hookSpecificOutput": {"additionalContext": "command note"}}))
        if event == "Tool":
            return types.SimpleNamespace(stdout='{"results":[]}')
        return types.SimpleNamespace(stdout='{"ok":true}')

    def timer(self, delay, callback):
        timer = Timer(delay, callback)
        self.timers.append(timer)
        return timer

    def events(self):
        return [event for event, _ in self.requests]


class CommandLine(Hermes):
    def test_plugin_load_registers_provider_and_tool_hooks_only(self):
        self.assertEqual(set(self.ctx.hooks), {"post_tool_call", "transform_tool_result", "on_session_end"})


    def test_provider_lifecycle_maps_to_cairn_events(self):
        provider = self.provider()
        self.assertEqual(provider.name, "cairn-memory")
        provider.on_turn_start(1, "first prompt")
        self.assertEqual(self.events(), ["SessionStart", "UserPromptSubmit"])
        self.assertTrue(all(body["cwd"] == "/repo" and body["session_id"] == "host-session"
                            for _, body in self.requests))
        self.assertEqual(self.requests[-1][1]["user_message"], "first prompt")
        self.assertEqual(provider.prefetch("first prompt"), "custom instruction\nrecall")
        self.assertEqual(provider.prefetch("again"), "")
        provider.on_turn_start(2, "second prompt")
        self.assertEqual(self.events()[-1], "UserPromptSubmit")
        self.assertEqual(provider.prefetch("second prompt"), "recall")

        provider.sync_turn("second prompt", "answer", session_id="host-session")
        self.assertEqual(self.requests[-1], ("Stop", {"session_id": "host-session", "cwd": "/repo",
                                                     "assistant_response": "answer"}))
        self.assertEqual(provider.on_pre_compress([{"role": "user", "content": "x"}]), "")
        self.assertEqual(self.requests[-1][0], "PreCompact")

        before = len(self.requests)
        provider.on_session_switch("compressed-child", parent_session_id="host-session", reset=False,
                                   reason="compression")
        provider.on_turn_start(3, "after compression")
        self.assertEqual(self.events()[before:], ["UserPromptSubmit"])
        self.assertEqual(self.requests[-1][1]["session_id"], "compressed-child")

        provider.on_session_switch("compressed-child", rewound=True)
        provider.on_session_end([])
        self.assertEqual(self.requests[-1], ("SessionEnd", {"session_id": "compressed-child", "cwd": "/repo"}))
        provider.on_session_switch("new-session", parent_session_id="compressed-child", reset=True,
                                   reason="new_session")
        provider.on_turn_start(1, "new conversation")
        self.assertEqual(self.events()[-2:], ["SessionStart", "UserPromptSubmit"])
        self.assertEqual(self.requests[-1][1]["session_id"], "new-session")
        for switch in [{"reset": False, "reason": "resume"}, {"reset": False, "reason": "branch"}]:
            provider.prefetch("")
            provider.on_session_switch("other-" + switch["reason"], **switch)
            provider.on_turn_start(1, "resumed")
            self.assertEqual(self.events()[-2:], ["SessionStart", "UserPromptSubmit"])

    def test_brief_waits_for_the_first_prefetch_after_a_trivial_prompt(self):
        provider = self.provider()
        provider.on_turn_start(1, "hi")  # Hermes skips prefetch for a trivial prompt.
        provider.on_turn_start(2, "real question")
        self.assertEqual(provider.prefetch("real question"), "custom instruction\nrecall")
        self.assertEqual(self.events().count("SessionStart"), 1)

    def test_tools_mirror_mcp_and_run_through_the_cli(self):
        provider = self.provider()
        expected = [{"name": tool["name"], "description": tool["description"], "parameters": tool["inputSchema"]}
                    for tool in TOOLS]
        self.assertEqual(provider.get_tool_schemas(), expected)
        self.assertEqual([tool["name"] for tool in expected], [
            "memory_search", "memory_get", "memory_save", "memory_feedback", "memory_forget",
            "skill_search", "skill_get", "skill_save", "skill_feedback"])
        for tool in TOOLS:
            args = {key: "test" for key in tool["inputSchema"].get("required", [])}
            self.assertEqual(json.loads(provider.handle_tool_call(tool["name"], args)), {"results": []})
            self.assertEqual(self.requests[-1], ("Tool", {"session_id": "host-session", "cwd": "/repo",
                                                         "tool_name": tool["name"], "arguments": args}))
        before = len(self.requests)
        self.assertIn("error", json.loads(provider.handle_tool_call("memory_link", {})))
        self.assertEqual(len(self.requests), before)

    def test_tool_hooks_capture_cairn_tools_as_mcp_and_attach_command_recall(self):
        hooks = self.ctx.hooks
        hooks["post_tool_call"](tool_name="memory_save", args={"kind": "note"}, result='{"results":[]}',
                                session_id="host-session", tool_call_id="call-1", status="ok")
        self.assertEqual(self.requests[-1][0], "PostToolUse")
        self.assertEqual(self.requests[-1][1]["tool_name"], "mcp_cairn_memory_save")
        self.assertEqual(self.requests[-1][1]["cwd"], "/stubbed-cwd")
        hooks["post_tool_call"](tool_name="web_search", args={}, result="{}", session_id="host-session")
        self.assertEqual(self.requests[-1][1]["tool_name"], "mcp_cairn_memory_save")
        hooks["post_tool_call"](tool_name="terminal", args={"command": "make"}, result='{"exit_code":2}',
                                session_id="host-session", tool_call_id="call-2", status="error")
        attached = hooks["transform_tool_result"](tool_name="terminal", result='{"exit_code":2}',
                                                  session_id="host-session", tool_call_id="call-2")
        self.assertEqual(json.loads(attached), {"exit_code": 2, "cairn_recall": "command note"})

        before = len(self.requests)
        hooks["on_session_end"](session_id="host-session", failed=False, interrupted=False)
        self.assertEqual(len(self.requests), before)
        for flag in ["failed", "interrupted"]:
            hooks["on_session_end"](session_id="host-session", **{flag: True})
            self.assertEqual(self.requests[-1], ("Stop", {"session_id": "host-session", "cwd": "/stubbed-cwd"}))

    def test_background_agents_neither_write_nor_get_tools(self):
        cases = [{"agent_context": "subagent"}, {"agent_context": "cron"}, {"agent_context": "flush"},
                 {"platform": "cron"}, {"platform": "subagent"}]
        sessions = ["host-session"] * len(cases) + ["cron_job", "flush_job"]
        for details, session in zip(cases + [{}, {}], sessions):
            provider = self.provider(session, **details)
            self.assertEqual(provider.get_tool_schemas(), [])
            self.assertIn("error", json.loads(provider.handle_tool_call("memory_save", {"kind": "note"})))
            provider.on_turn_start(1, "skip")
            self.assertEqual(provider.prefetch("skip"), "")
            provider.sync_turn("skip", "skip")
            provider.on_pre_compress([])
            provider.on_session_end([])
        live = self.provider("host-session")
        self.delegated = True
        live.on_turn_start(1, "delegated")
        for hook in self.ctx.hooks.values():
            hook(tool_name="terminal", args={}, result="{}", session_id="host-session", failed=True)
        self.bound["HERMES_CRON_SESSION"] = "1"
        self.delegated = False
        live.on_turn_start(1, "cron")
        self.ctx.hooks["post_tool_call"](tool_name="terminal", args={}, result="{}", session_id="host-session",
                                         task_id="cron:job:run")
        self.assertEqual(self.requests, [])
        self.bound.clear()
        older_hermes = self.provider("live", agent_context=None)
        older_hermes.on_turn_start(1, "interactive")
        self.assertEqual(older_hermes.prefetch("interactive"), "custom instruction\nrecall")


class Gateway(Hermes):
    gateway = True

    def route(self, platform="telegram", chat="chat-transport-id", kind="dm", actor="user-owner", thread="",
              parent="", session="host-session"):
        self.bound = {"PLATFORM": platform, "CHAT_ID": chat, "CHAT_TYPE": kind, "ACTOR_ID": actor,
                      "THREAD_ID": thread, "PARENT_CHAT_ID": parent, "USER_ID": "user-owner", "ID": session}

    def turn(self, provider, message="A user prompt"):
        provider.on_turn_start(1, message)
        return provider.prefetch(message)

    def end(self, session="host-session"):
        self.ctx.hooks["on_session_end"](session)

    def test_chat_tools_need_a_route_and_send_only_hashed_scope(self):
        self.assertEqual(self.ctx.tools, {})
        provider = self.provider()
        schemas = provider.get_tool_schemas()
        self.assertEqual([schema["name"] for schema in schemas],
                         ["memory_search", "memory_save", *(tool["name"] for tool in SKILL_TOOLS)])
        for skill in SKILL_TOOLS:
            self.assertIn({"name": skill["name"], "description": skill["description"],
                           "parameters": skill["inputSchema"]}, schemas)
        self.assertEqual(self.turn(provider), "")
        self.assertIn("error", json.loads(provider.handle_tool_call("memory_search", {"query": "needle"})))
        for platform, chat, kind in [("UPPER", "chat", "dm"), ("x" * 33, "chat", "dm"),
                                     ("matrix", "", "dm"), ("matrix", "chat", "")]:
            self.route(platform, chat, kind)
            self.assertEqual(self.turn(provider), "")
        self.assertEqual(self.requests, [])

        self.route("new_platform-2")
        self.assertEqual(self.turn(provider), "custom instruction\nrecall")
        self.assertEqual(self.turn(provider), "recall")
        provider.handle_tool_call("memory_search", {"query": "needle", "project": "E2E Hermes", "owner": "sender-id"})
        event, request = self.requests[-1]
        self.assertEqual(event, "ChatSearch")
        self.assertEqual(request["chat_context"], {
            "platform": "new_platform-2", "kind": "dm", "thread_id_hash": None,
            "chat_id_hash": self.plugin["chat_digest"](self.salt, "new_platform-2", "chat", "chat-transport-id"),
        })
        self.assertEqual(request["arguments"], {"query": "needle", "project": "E2E Hermes"})
        for skill in SKILL_TOOLS:
            args = {key: "test" for key in skill["inputSchema"]["required"]}
            provider.handle_tool_call(skill["name"], dict(args, project="unwanted", context={"chat": "raw"}))
            event, request = self.requests[-1]
            self.assertEqual((event, request["skill_tool"], request["arguments"]), ("ChatSkill", skill["name"], args))
        provider.handle_tool_call("memory_save", {"kind": "note", "title": "t", "summary": "s", "sender": "raw"})
        self.assertEqual(self.requests[-1][0], "ChatSave")
        self.assertEqual(self.requests[-1][1]["arguments"], {"kind": "note", "title": "t", "summary": "s"})
        transport = json.dumps(self.requests)
        for value in ["chat-transport-id", "user-owner", "sender-id", "sender_id_hash", "project_id"]:
            self.assertNotIn(value, transport)
        self.route(session="another-session")
        before = len(self.requests)
        self.assertIn("error", json.loads(provider.handle_tool_call("memory_search", {"query": "needle"})))
        self.assertEqual(len(self.requests), before)

    def test_chats_dms_groups_and_discord_threads_get_their_own_scope(self):
        scopes = {}
        for name, route in [("dm", ("telegram", "chat-a", "dm")), ("other dm", ("telegram", "chat-b", "dm")),
                            ("group", ("telegram", "group-c", "group"))]:
            self.route(*route, session=name)
            self.turn(self.provider(name))
            scopes[name] = self.requests[-1][1]["chat_context"]
            self.end(name)
        self.assertEqual(scopes["dm"]["kind"], "dm")
        self.assertEqual(scopes["group"]["kind"], "group")
        self.assertNotEqual(scopes["dm"]["chat_id_hash"], scopes["other dm"]["chat_id_hash"])

        self.route("discord", "raw-thread-1", "thread", "actor1", "raw-thread-1", "raw-channel", "t1")
        self.turn(self.provider("t1"))
        thread = self.requests[-1][1]["chat_context"]
        self.route("discord", "raw-channel", "group", "actor1", session="channel")
        self.turn(self.provider("channel"))
        channel = self.requests[-1][1]["chat_context"]
        self.assertEqual(thread["chat_id_hash"], channel["chat_id_hash"])
        self.assertEqual(thread["chat_id_hash"],
                         self.plugin["chat_digest"](self.salt, "discord", "chat", "raw-channel"))
        self.assertIsNotNone(thread["thread_id_hash"])
        self.assertIsNone(channel["thread_id_hash"])
        self.assertEqual(thread["kind"], "group")
        for raw in ["raw-channel", "raw-thread-", "chat-a", "group-c", "user-owner"]:
            self.assertNotIn(raw, json.dumps(self.requests))

    def test_threads_share_a_chat_tag_but_have_separate_idle_sessions(self):
        self.route("discord", "raw-thread-1", "thread", "actor1", "raw-thread-1", "raw-channel")
        self.turn(self.provider())
        first = self.requests[-1][1]
        self.end()
        self.assertEqual(self.timers[-1].delay, 1800)
        old_timer = self.timers[-1]
        self.route("discord", "raw-thread-1", "thread", "actor2", "raw-thread-1", "raw-channel", "another-host-session")
        self.turn(self.provider("another-host-session"))
        same = self.requests[-1][1]
        self.assertEqual(first["session_id"], same["session_id"])
        self.assertEqual(first["chat_context"], same["chat_context"])
        self.assertTrue(old_timer.cancelled)
        old_timer.callback()
        self.assertEqual(self.requests[-1][0], "UserPromptSubmit")
        self.end("another-host-session")
        idle = self.timers[-1]

        self.route("discord", "raw-thread-2", "thread", "actor1", "raw-thread-2", "raw-channel")
        self.turn(self.provider())
        second = self.requests[-1][1]
        self.assertEqual(first["chat_context"]["chat_id_hash"], second["chat_context"]["chat_id_hash"])
        self.assertNotEqual(first["chat_context"]["thread_id_hash"], second["chat_context"]["thread_id_hash"])
        self.assertNotEqual(first["session_id"], second["session_id"])

        idle.callback()
        self.assertEqual(self.events()[-2:], ["Stop", "SessionEnd"])
        self.assertEqual(self.requests[-1][1]["session_id"], first["session_id"])
        self.assertEqual(self.requests[-1][1]["chat_context"], same["chat_context"])
        self.route("discord", "raw-thread-1", "thread", "actor1", "raw-thread-1", "raw-channel")
        self.assertEqual(self.turn(self.provider()), "custom instruction\nrecall")
        self.assertNotEqual(first["session_id"], self.requests[-1][1]["session_id"])

    def finalize(self, session="host-session", reason="new_session", new="fresh"):
        # Hermes runs finalize off the turn: no session context is bound.
        bound, self.bound = self.bound, {}
        self.ctx.hooks["on_session_finalize"](session_id=session, platform="telegram", reason=reason,
                                              old_session_id=session, new_session_id=new)
        self.bound = bound

    def test_new_or_reset_tears_down_the_agent_and_the_next_prompt_starts_a_chat_session(self):
        self.route()
        provider = self.provider()
        self.turn(provider)
        first = self.requests[-1][1]["session_id"]
        self.end()
        # Ordinary eviction shuts the provider down and builds a new one: the chat session continues.
        provider.on_session_end([])
        provider = self.provider()
        self.assertEqual(self.turn(provider), "recall")
        self.assertEqual(self.requests[-1][1]["session_id"], first)
        self.end()
        timer = self.timers[-1]
        self.assertNotIn("SessionEnd", self.events())

        # /new: Hermes tears the agent down, finalizes the old host session, then builds a new provider.
        provider.on_session_end([])
        self.finalize()
        self.assertEqual(self.events()[-2:], ["Stop", "SessionEnd"])
        self.assertEqual(self.requests[-1][1]["session_id"], first)
        self.assertTrue(timer.cancelled)
        self.finalize()
        self.assertEqual(self.events().count("SessionEnd"), 1)
        self.route(session="fresh")
        provider = self.provider("fresh")
        self.assertEqual(self.turn(provider), "custom instruction\nrecall")
        second = self.requests[-1][1]["session_id"]
        self.assertNotEqual(second, first)
        before = len(self.requests)
        provider.sync_turn("prompt", "answer")
        provider.on_pre_compress([])
        provider.on_session_end([])
        self.assertEqual(len(self.requests), before, "chat sessions end by idle time or reset only")

        # A reset during a running turn waits for that turn to end.
        self.finalize("fresh", new="third")
        self.assertEqual(self.events()[-1], "UserPromptSubmit")
        self.end("fresh")
        self.assertEqual(self.events()[-2:], ["Stop", "SessionEnd"])
        self.assertEqual(self.requests[-1][1]["session_id"], second)
        self.assertEqual(len(self.timers), 2, "an ended chat session arms no idle timer")

        # Shutdown finalizes too; a turn that never reports its end closes at the next prompt.
        self.route(session="third")
        provider = self.provider("third")
        self.turn(provider)
        third = self.requests[-1][1]["session_id"]
        self.finalize("third", reason="shutdown", new=None)
        self.route(session="fourth")
        self.assertEqual(self.turn(self.provider("fourth")), "custom instruction\nrecall")
        self.assertEqual([(event, body["session_id"]) for event, body in self.requests[-4:-2]],
                         [("Stop", third), ("SessionEnd", third)])
        self.end("third")
        self.assertNotEqual(self.events()[-1], "SessionEnd")

    def test_in_place_reset_switch_ends_the_chat_session_after_the_turn(self):
        self.route()
        provider = self.provider()
        self.turn(provider)
        first = self.requests[-1][1]["session_id"]
        provider.on_session_switch("host-session", rewound=True)
        provider.on_session_switch("host-session", reset=False, reason="resume")
        self.assertNotIn("SessionEnd", self.events())
        provider.on_session_switch("fresh", parent_session_id="host-session", reset=True)
        self.assertNotIn("SessionEnd", self.events(), "a running turn keeps its chat session")
        self.end()
        self.assertEqual(self.events()[-2:], ["Stop", "SessionEnd"])
        self.assertEqual(self.requests[-1][1]["session_id"], first)
        self.route(session="fresh")
        self.assertEqual(self.turn(provider), "custom instruction\nrecall")
        self.assertNotEqual(self.requests[-1][1]["session_id"], first)

    def test_terminal_recall_and_background_turns(self):
        self.route()
        provider = self.provider()
        self.turn(provider)
        hooks = self.ctx.hooks
        hooks["post_tool_call"](tool_name="read_file", args={}, result="{}", session_id="host-session")
        self.assertNotEqual(self.events()[-1], "PostToolUse")
        hooks["post_tool_call"](tool_name="terminal", args={"command": "make"}, result='{"exit_code":2}',
                                session_id="host-session", tool_call_id="call-1", status="error")
        self.assertIn("chat_context", self.requests[-1][1])
        attached = hooks["transform_tool_result"](tool_name="terminal", result='{"exit_code":2}',
                                                  session_id="host-session", tool_call_id="call-1")
        self.assertEqual(json.loads(attached)["cairn_recall"], "command note")

        before = len(self.requests)
        for details in [{"agent_context": "subagent"}, {"platform": "cron"}, {"agent_context": "flush"}]:
            background = self.provider(**details)
            self.assertEqual(background.get_tool_schemas(), [])
            self.assertEqual(self.turn(background), "")
        self.delegated = True
        self.assertEqual(self.turn(provider), "")
        hooks["post_tool_call"](tool_name="terminal", args={}, result="{}", session_id="host-session")
        self.assertIn("error", json.loads(provider.handle_tool_call("memory_save", {"kind": "note"})))
        self.delegated = False
        self.bound["HERMES_CRON_SESSION"] = "1"
        self.assertEqual(self.turn(provider), "")
        self.assertEqual(len(self.requests), before)

    def test_provider_without_the_plugin_load_keeps_chat_memory_off(self):
        sys.modules.pop("_cairn_memory_hermes", None)
        self.route()
        provider = self.provider()
        # A gateway process that started before setup: no tools, and a stale schema gets the restart hint.
        self.assertEqual(provider.get_tool_schemas(), [])
        self.assertEqual(self.turn(provider), "")
        error = json.loads(provider.handle_tool_call("memory_search", {"query": "x"}))["error"]
        self.assertIn("Start the Hermes gateway process again", error)
        self.assertEqual(self.requests, [])


if __name__ == "__main__":
    unittest.main()
