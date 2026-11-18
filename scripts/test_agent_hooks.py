#!/usr/bin/env python3
"""Agent lifecycle uses loopback stubs and a temporary home; no live API."""
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.server.calls.append((self.path, body))
        data = json.dumps({'brief':'Cairn memory data\n```json\n{}\n```'} if self.path.endswith('/brief') else {'accepted':True}).encode()
        self.send_response(getattr(self.server, "status", 200))
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *_): pass

class AgentHooks(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.calls = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.home.cleanup()
    def hook(self, event, agent='claude', stored=False, capture_assistant=False, **extra):
        env = dict(os.environ, HOME=self.home.name, XDG_CONFIG_HOME=self.home.name,
                   CAIRN_DEVICE_TOKEN='test-device', CAIRN_URL=f'http://127.0.0.1:{self.server.server_port}')
        if stored:
            config = Path(self.home.name, 'cairn/config.json')
            config.parent.mkdir(exist_ok=True)
            config.write_text(json.dumps({'url':env.pop('CAIRN_URL'), 'token':env.pop('CAIRN_DEVICE_TOKEN')}))
        if capture_assistant: env['CAIRN_CAPTURE_ASSISTANT']='1'
        else: env.pop('CAIRN_CAPTURE_ASSISTANT', None)
        env.pop('CAIRN_OUTBOX', None)
        env.pop('CAIRN_SESSION_ID', None)
        payload = dict(session_id='session', hook_event_name=event, cwd=self.home.name, **extra)
        result = subprocess.run([sys.executable, str(ROOT/'scripts/cairn_hook.py'), '--agent', agent], input=json.dumps(payload), text=True, capture_output=True, env=env, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout) if result.stdout.strip() else {}
    def test_start_short_first_work_once_and_end_only_after_new_work(self):
        for agent in ['claude', 'codex']:
            with self.subTest(agent=agent):
                start = self.hook('SessionStart', agent)
                text = start['hookSpecificOutput']['additionalContext']
                self.assertIn('memory tools', text)
                self.assertIn('```json', text)
                self.assertNotIn('alternatives', text)
                self.assertLessEqual(len(text.encode()), 4352)
                self.assertEqual(self.hook('Stop', agent), {})
                self.assertEqual(self.hook('PostToolUse', agent, tool_name='Read'), {})
                first = self.hook('PostToolUse', agent, tool_name='Bash', tool_use_id='1', tool_input={'command':'test'})
                text = first['hookSpecificOutput']['additionalContext']
                for field in ['scope', 'title', 'summary', 'reason', 'alternatives', 'exceptions', 'symptom', 'steps', 'statement', 'preference']:
                    self.assertIn(field, text)
                self.assertLessEqual(len(text.encode()), 2048)
                self.assertEqual(self.hook('PostToolUse', agent, tool_name='Edit', tool_use_id='2'), {})
                ask = self.hook('Stop', agent)
                self.assertEqual(ask['decision'], 'block')
                self.assertIn('worth remembering', ask['reason'])
                self.assertEqual(self.hook('Stop', agent), {})
                self.hook('PostToolUse', agent, tool_name='Bash', tool_use_id='3')
                self.assertEqual(self.hook('Stop', agent, stop_hook_active=True), {})
                self.assertEqual(self.hook('Stop', agent), {})
                self.hook('PostToolUse', agent, tool_name='apply_patch', tool_use_id='4')
                self.assertEqual(self.hook('Stop', agent)['decision'], 'block')
                self.hook('SessionEnd', agent)
                self.assertEqual(self.hook('Stop', agent), {})
        self.assertEqual(sum(path.endswith('/brief') for path, _ in self.server.calls), 2)
    def test_resume_keeps_once_flag_and_concurrent_first_work_injects_once(self):
        self.hook('SessionStart')
        with ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(lambda n:self.hook('PostToolUse', tool_name='Bash', tool_use_id=str(n)), range(8)))
        self.assertEqual(sum(bool(result) for result in results), 1)
        self.hook('SessionStart', source='resume')
        self.assertEqual(self.hook('PostToolUse', tool_name='Write'), {})
    def test_offline_capture_does_not_discard_first_work_instructions(self):
        self.server.status = 503
        start = self.hook('SessionStart')
        self.assertNotIn('```json', start['hookSpecificOutput']['additionalContext'])
        result = self.hook('PostToolUse', tool_name='Bash', tool_use_id='offline')
        self.assertIn('required', result['hookSpecificOutput']['additionalContext'].lower())
        self.assertEqual(self.hook('Stop')['decision'], 'block')

    def test_agent_stop_preserves_explicit_assistant_capture_opt_in(self):
        self.assertEqual(self.hook('Stop', last_assistant_message='assistant text'), {})
        self.assertEqual(self.server.calls, [])
        self.assertEqual(self.hook('Stop', capture_assistant=True, last_assistant_message='assistant text'), {})
        self.assertEqual(self.server.calls[-1][0], '/cairn-memory/sync/push')
        self.assertEqual(self.server.calls[-1][1]['role'], 'assistant')

    def test_configured_credentials_work_without_environment_override(self):
        start = self.hook('SessionStart', stored=True)
        self.assertIn('```json', start['hookSpecificOutput']['additionalContext'])
        self.assertEqual(self.server.calls[0][0], '/cairn-memory/session/brief')

    def test_once_flags_survive_many_open_sessions_without_payload_or_token(self):
        import cairn_hook
        import sqlite3
        config = Path(self.home.name, 'cairn/config.json')
        for number in range(260):
            payload = dict(session_id=str(number), hook_event_name='PostToolUse', tool_name='Bash', tool_input={'command':'private-payload'}, cwd=self.home.name)
            cairn_hook.agent_context(payload, 'claude', config, 'private-test-token', 'http://127.0.0.1:9')
        payload['session_id']='0'
        output, _ = cairn_hook.agent_context(payload, 'claude', config, 'private-test-token', 'http://127.0.0.1:9')
        self.assertEqual(output, {})
        path = config.parent/'hook-state.sqlite3'
        with sqlite3.connect(path) as db:
            rows = str(db.execute('SELECT * FROM sessions').fetchall())
        self.assertNotIn('private-test-token', rows)
        self.assertNotIn('private-payload', rows)

    def test_marker_disables_agent_hook_without_network_or_state(self):
        Path(self.home.name, '.cairnignore').touch()
        for event in ['SessionStart', 'PostToolUse', 'Stop', 'SessionEnd']:
            self.assertEqual(self.hook(event, tool_name='Bash'), {})
        self.assertEqual(self.server.calls, [])
        self.assertEqual(list(Path(self.home.name).rglob('*.sqlite3')), [])

if __name__ == '__main__': unittest.main()
