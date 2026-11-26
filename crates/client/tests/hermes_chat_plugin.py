import json
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch

PLUGIN = Path(__file__).resolve().parents[1] / "src/setup/hermes/plugin.py"


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
        gateway = types.ModuleType("gateway.session_context")
        gateway.bound_session_env = lambda key: self.bound.get(key.removeprefix("HERMES_SESSION_"), "")
        registry = types.ModuleType("tools.registry")
        registry.no_cache_check_fn = lambda check: check
        modules = patch.dict(sys.modules, {"gateway.session_context": gateway, "tools.registry": registry})
        modules.start()
        self.addCleanup(modules.stop)
        namespace = {}
        script = PLUGIN.read_text().replace("__CAIRN_PATHS__", '["cairn", "selected.json"]')
        exec(compile(script.replace("__CAIRN_GATEWAY__", "True"), str(PLUGIN), "exec"), namespace)
        for target, replacement in [("subprocess.run", self.run_hook), ("threading.Timer", self.timer)]:
            module, name = target.split(".")
            stub = patch.object(namespace[module], name, replacement)
            stub.start()
            self.addCleanup(stub.stop)
        self.digest = namespace["chat_digest"]
        self.salt = "test-only-salt"
        self.ctx = Context({
            "chat_salt": self.salt,
            "chat_owner_hashes": {"telegram": self.digest(self.salt, "telegram", "sender", "user-owner")},
        })
        namespace["register"](self.ctx)

    def run_hook(self, command, input, **_):
        request = json.loads(input)
        self.requests.append((command[2], request))
        if command[2] in {"SessionStart", "UserPromptSubmit"}:
            return types.SimpleNamespace(stdout='{"hookSpecificOutput":{"additionalContext":"recall"}}')
        return types.SimpleNamespace(stdout='{"ok":true}')

    def timer(self, delay, callback):
        timer = Timer(delay, callback)
        self.timers.append(timer)
        return timer

    def route(self, platform="telegram", chat="chat-transport-id", kind="dm", actor="user-owner", thread="",
              parent=""):
        self.bound = {"PLATFORM": platform, "CHAT_ID": chat, "CHAT_TYPE": kind, "ACTOR_ID": actor,
                      "THREAD_ID": thread, "PARENT_CHAT_ID": parent, "USER_ID": "user-owner"}

    def prompt(self, host_session="host-session"):
        return self.ctx.hooks["pre_llm_call"](host_session, "A user prompt")

    def end(self, host_session="host-session"):
        self.ctx.hooks["on_session_end"](host_session)

    def test_owner_and_missing_actor_use_transport_context(self):
        self.assertEqual(set(self.ctx.tools), {"memory_search", "memory_save"})
        self.assertTrue(all(tool["toolset"] == "cairn-chat" for tool in self.ctx.tools.values()))
        self.assertIsNone(self.prompt())
        self.assertEqual(self.requests, [])
        self.assertTrue(all(not tool["check_fn"]() for tool in self.ctx.tools.values()))
        self.route()
        self.assertEqual(self.prompt(), {"context": "recall\nrecall"})
        tool = self.ctx.tools["memory_search"]
        self.assertTrue(tool["check_fn"]())
        tool["handler"]({"query": "needle", "chat_context": {"owner": False}})
        event, request = self.requests[-1]
        self.assertEqual(event, "ChatSearch")
        self.assertTrue(request["chat_context"]["owner"])
        self.assertEqual(request["chat_context"]["kind"], "dm")
        self.assertEqual(request["chat_context"]["sender_id_hash"],
                         self.digest(self.salt, "telegram", "sender", "user-owner"))
        for actor in ["unmapped-user", ""]:
            self.route(actor=actor)
            self.prompt()
            self.ctx.tools["memory_save"]["handler"]({"kind": "note"})
            request = self.requests[-1][1]
            self.assertFalse(request["chat_context"]["owner"])
            if not actor:
                self.assertIsNone(request["chat_context"]["sender_id_hash"])
        self.assertNotIn("chat-transport-id", json.dumps(self.requests))
        self.assertNotIn("user-owner", json.dumps(self.requests))

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
        self.assertNotEqual(first["chat_context"]["sender_id_hash"], same["chat_context"]["sender_id_hash"])
        self.assertEqual(same["chat_context"]["kind"], "group")
        self.assertFalse(same["chat_context"]["owner"])
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
        self.prompt()
        self.assertNotEqual(first["session_id"], self.requests[-1][1]["session_id"])
        self.assertNotIn("raw-channel", json.dumps(self.requests))
        self.assertNotIn("raw-thread-", json.dumps(self.requests))


if __name__ == "__main__":
    unittest.main()
