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
    def __init__(self, settings):
        self.settings = settings
        self.hooks = {}
        self.tools = {}

    def get_config(self, key, default=None):
        return self.settings.get(key, default)

    def register_hook(self, name, callback):
        self.hooks[name] = callback

    def register_tool(self, **tool):
        self.tools[tool["name"]] = tool

    def session_cwd(self):
        return "/stubbed-cwd"


class Timer:
    def __init__(self, delay, callback):
        self.delay = delay
        self.callback = callback
        self.cancelled = False

    def start(self):
        pass

    def cancel(self):
        self.cancelled = True


class GatewayPlugin(unittest.TestCase):
    def setUp(self):
        self.bound = {}
        self.requests = []
        self.timers = []
        self.delegated = False
        home = tempfile.TemporaryDirectory(prefix="cairn-hermes-test-")
        self.addCleanup(home.cleanup)
        env = patch.dict("os.environ", {"HERMES_HOME": home.name})
        env.start()
        self.addCleanup(env.stop)
        gateway = types.ModuleType("gateway.session_context")
        gateway.bound_session_env = lambda key: self.bound.get(key.removeprefix("HERMES_SESSION_"), "")
        registry = types.ModuleType("tools.registry")
        registry.no_cache_check_fn = lambda check: check
        delegation = types.ModuleType("agent.delegation_context")
        delegation.is_delegated_child_process_context = lambda: self.delegated
        modules = patch.dict(sys.modules, {
            "gateway.session_context": gateway, "tools.registry": registry,
            "agent.delegation_context": delegation,
        })
        modules.start()
        self.addCleanup(modules.stop)
        namespace = self.load_plugin(True)
        for target, replacement in [("subprocess.run", self.run_hook), ("threading.Timer", self.timer)]:
            module, name = target.split(".")
            stub = patch.object(namespace[module], name, replacement)
            stub.start()
            self.addCleanup(stub.stop)
        self.digest = namespace["chat_digest"]
        self.salt = "test-only-salt"
        self.ctx = Context({"chat_salt": self.salt})
        namespace["register"](self.ctx)

    def load_plugin(self, gateway):
        namespace = {}
        script = PLUGIN.read_text().replace("__CAIRN_PATHS__", '["cairn", "selected.json"]')
        script = script.replace("__CAIRN_GATEWAY__", str(gateway)).replace(
            "__CAIRN_SKILL_TOOLS__", json.dumps(json.dumps(SKILL_TOOLS))
        )
        exec(compile(script, str(PLUGIN), "exec"), namespace)
        return namespace

    def run_hook(self, command, input, **_):
        request = json.loads(input)
        self.requests.append((command[2], request))
        if command[2] in {"SessionStart", "UserPromptSubmit"}:
            note = "custom instruction" if command[2] == "SessionStart" else "recall"
            return types.SimpleNamespace(stdout=json.dumps({"hookSpecificOutput": {"additionalContext": note}}))
        return types.SimpleNamespace(stdout='{"ok":true}')

    def timer(self, delay, callback):
        timer = Timer(delay, callback)
        self.timers.append(timer)
        return timer

    def route(self, platform="telegram", chat="chat-transport-id", kind="dm", actor="user-owner", thread="",
              parent=""):
        self.bound = {"PLATFORM": platform, "CHAT_ID": chat, "CHAT_TYPE": kind, "ACTOR_ID": actor,
                      "THREAD_ID": thread, "PARENT_CHAT_ID": parent, "USER_ID": "user-owner"}

    def prompt(self, host_session="host-session", **details):
        return self.ctx.hooks["pre_llm_call"](session_id=host_session, user_message="A user prompt", **details)

    def end(self, host_session="host-session"):
        self.ctx.hooks["on_session_end"](host_session)

    def test_all_skill_tools_filter_transport_and_project_arguments(self):
        self.route(kind="group")
        self.prompt()
        for skill in SKILL_TOOLS:
            name = skill["name"]
            tool = self.ctx.tools[name]
            self.assertEqual(tool["schema"]["parameters"], skill["inputSchema"])
            self.assertTrue(tool["check_fn"]())
            args = {key: "test" for key in skill["inputSchema"]["required"]}
            tool["handler"](dict(args, project="E2E unwanted", context={"chat": "raw"}, sender="raw"))
            event, request = self.requests[-1]
            self.assertEqual(event, "ChatSkill")
            self.assertEqual(request["skill_tool"], name)
            self.assertEqual(request["arguments"], args)

    def test_only_hashed_routes_and_explicit_project_reach_cairn(self):
        self.assertEqual(set(self.ctx.tools), {"memory_search", "memory_save", *(tool["name"] for tool in SKILL_TOOLS)})
        self.assertTrue(all(tool["toolset"] == "cairn-chat" for tool in self.ctx.tools.values()))
        self.assertIsNone(self.prompt())
        self.assertEqual(self.requests, [])
        self.assertTrue(all(not tool["check_fn"]() for tool in self.ctx.tools.values()))
        for platform, chat, kind in [("UPPER", "chat", "dm"), ("x" * 33, "chat", "dm"),
                                     ("matrix", "", "dm"), ("matrix", "chat", "")]:
            self.route(platform, chat, kind)
            self.assertIsNone(self.prompt())
        self.route("new_platform-2")
        self.assertEqual(self.prompt(), {"context": "custom instruction\nrecall"})
        self.assertEqual(self.prompt(), {"context": "recall"})
        tool = self.ctx.tools["memory_search"]
        self.assertTrue(tool["check_fn"]())
        tool["handler"]({"query": "needle", "project": "E2E Hermes", "owner": "sender-id"})
        event, request = self.requests[-1]
        self.assertEqual(event, "ChatSearch")
        self.assertEqual(request["chat_context"], {
            "platform": "new_platform-2", "kind": "dm", "thread_id_hash": None,
            "chat_id_hash": self.digest(self.salt, "new_platform-2", "chat", "chat-transport-id"),
        })
        self.assertEqual(request["arguments"], {"query": "needle", "project": "E2E Hermes"})
        self.assertTrue(all("project_id" not in body for _, body in self.requests))
        transport = json.dumps(self.requests)
        for value in ["chat-transport-id", "user-owner", "sender-id", "sender_id_hash"]:
            self.assertNotIn(value, transport)
        self.bound.clear()
        before = len(self.requests)
        self.assertIn("error", json.loads(tool["handler"]({"query": "needle"})))
        self.assertEqual(len(self.requests), before)

    def test_threads_share_a_chat_tag_but_have_separate_idle_sessions(self):
        self.route("discord", "raw-thread-1", "thread", "actor1", "raw-thread-1", "raw-channel")
        self.prompt()
        first = self.requests[-1][1]
        self.end()
        self.assertEqual(self.timers[-1].delay, 1800)
        old_timer = self.timers[-1]
        self.route("discord", "raw-thread-1", "thread", "actor2", "raw-thread-1", "raw-channel")
        self.prompt("another-host-session")
        same = self.requests[-1][1]
        self.assertEqual(first["session_id"], same["session_id"])
        self.assertEqual(first["chat_context"], same["chat_context"])
        self.assertEqual(same["chat_context"]["kind"], "group")
        self.assertTrue(old_timer.cancelled)
        old_timer.callback()
        self.assertEqual(self.requests[-1][0], "UserPromptSubmit")
        self.end("another-host-session")
        idle = self.timers[-1]

        self.route("discord", "raw-thread-2", "thread", "actor1", "raw-thread-2", "raw-channel")
        self.prompt()
        second = self.requests[-1][1]
        self.assertEqual(first["chat_context"]["chat_id_hash"], second["chat_context"]["chat_id_hash"])
        self.assertNotEqual(first["chat_context"]["thread_id_hash"], second["chat_context"]["thread_id_hash"])
        self.assertNotEqual(first["session_id"], second["session_id"])

        idle.callback()
        self.assertEqual([event for event, _ in self.requests[-2:]], ["Stop", "SessionEnd"])
        self.assertEqual(self.requests[-1][1]["session_id"], first["session_id"])
        self.assertEqual(self.requests[-1][1]["chat_context"], same["chat_context"])
        self.route("discord", "raw-thread-1", "thread", "actor1", "raw-thread-1", "raw-channel")
        self.assertEqual(self.prompt(), {"context": "custom instruction\nrecall"})
        self.assertNotEqual(first["session_id"], self.requests[-1][1]["session_id"])
        self.assertNotIn("raw-channel", json.dumps(self.requests))
        self.assertNotIn("raw-thread-", json.dumps(self.requests))

    def test_background_hooks_and_saves_are_skipped_in_both_modes(self):
        cli = Context({})
        self.load_plugin(False)["register"](cli)
        self.route()
        self.prompt()
        self.bound["ID"] = "host-session"
        before = len(self.requests)
        cases = [({"platform": "cron"}, False, ""), ({"platform": "subagent"}, False, ""),
                 ({"agent_context": "subagent"}, False, ""), ({"agent_context": "flush"}, False, ""),
                 ({"session_id": "cron_job"}, False, ""), ({"session_id": "flush_job"}, False, ""),
                 ({"task_id": "cron:job:run"}, False, ""), ({}, True, ""), ({}, False, "1")]
        for details, delegated, cron in cases:
            self.delegated = delegated
            self.bound["HERMES_CRON_SESSION"] = cron
            fields = {"session_id": "host-session", **details}
            for ctx in [cli, self.ctx]:
                self.assertIsNone(ctx.hooks["pre_llm_call"](user_message="skip", is_first_turn=True, **fields))
                self.assertIsNone(ctx.hooks["post_tool_call"](tool_name="terminal", args={}, result="{}", **fields))
                ctx.hooks["on_session_end"](failed=True, **fields)
            cli.hooks["post_llm_call"](assistant_response="skip", **fields)
            cli.hooks["post_compaction"](**fields)
            cli.hooks["on_session_finalize"](**fields)
            directive = cli.hooks["pre_tool_call"](tool_name="mcp__cairn__memory_save", **fields)
            self.assertEqual(directive["action"], "block")
            save = self.ctx.tools["memory_save"]["handler"]
            self.assertIn("error", json.loads(save({"kind": "note"}, **fields)))
        self.delegated = False
        del self.bound["HERMES_CRON_SESSION"]
        self.assertIn("error", json.loads(save({"kind": "note"}, session_id="different-session")))
        self.assertEqual(len(self.requests), before)
        self.assertEqual(self.timers, [])
        self.assertIsNone(cli.hooks["pre_tool_call"](tool_name="mcp__cairn__memory_save", session_id="live"))
        self.assertEqual(cli.hooks["pre_llm_call"](session_id="live", user_message="interactive", is_first_turn=True),
                         {"context": "custom instruction\nrecall"})


if __name__ == "__main__":
    unittest.main()
