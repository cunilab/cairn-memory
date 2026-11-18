#!/usr/bin/env python3
"""Config tests use temporary homes and native-CLI stand-ins only."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import cairn_setup as setup

class JsoncRemoval(unittest.TestCase):
    def test_comments_trailing_commas_nested_strings_and_other_bytes(self):
        text = r'''{
 // before
 "mcp": { "servers": {
    "other": {"command": ["{ // /* */ }", "\\\""], "nested": {"ok": true}},
    /* keep this */ "cairn": {"environment":{"CAIRN_SETUP_MANAGED":"1"},"command":["cairn", "mcp"],}, // keep that
    "last": {"url":"https://x.test/a//b/*c*/"},
 }}, // end
}'''
        node = setup.jsonc(text)
        result = setup.remove_cairn(text, node, allow_trailing=True)
        self.assertNotIn('"cairn"', result)
        for comment in ['// before', '/* keep this */', '// keep that', '// end']:
            self.assertIn(comment, result)
        self.assertEqual(setup.jsonc(result).value['mcp']['servers'], {
            'other':node.value['mcp']['servers']['other'], 'last':node.value['mcp']['servers']['last']})
        # The change is exactly the owned key/value and its separator.
        member = node.members['mcp'].members['servers'].members['cairn']
        self.assertEqual(result, text[:member.key_start] + text[member.end:member.comma[0]] + text[member.comma[1]:])
    def test_last_first_only_and_missing_entries_are_valid_json(self):
        own = {'environment': {setup.MARKER:'1'}, 'command':['cairn','mcp']}
        for items in [{'other':{}, 'cairn':own}, {'cairn':own,'other':{}}, {'cairn':own}, {'other':{}}]:
            text = json.dumps({'mcp':{'servers':items}})
            result = setup.remove_cairn(text, setup.jsonc(text), allow_trailing=False)
            expected = dict(items); expected.pop('cairn', None)
            self.assertEqual(json.loads(result)['mcp']['servers'], expected)
            if 'cairn' not in items: self.assertEqual(result, text)
    def test_unmarked_same_name_refused_even_on_remove(self):
        text = '{"mcp":{"servers":{"cairn":{"command":["my-server"]}}}}'
        with self.assertRaisesRegex(ValueError, 'unmarked'):
            setup.remove_cairn(text, setup.jsonc(text), allow_trailing=False)
    def test_duplicate_and_malformed_input_fail_closed(self):
        for text in ['{"a":1,"a":2}', '{/* unclosed', '{"a":}', '{"a":1} trailing', '{} /*unclosed']:
            with self.subTest(text=text), self.assertRaises(ValueError): setup.jsonc(text)

class Installer(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.home = Path(self.temp.name)
        env = dict(os.environ, HOME=str(self.home), USERPROFILE=str(self.home), XDG_CONFIG_HOME=str(self.home/'.config'), XDG_DATA_HOME=str(self.home/'.data'), XDG_CACHE_HOME=str(self.home/'.cache'), XDG_STATE_HOME=str(self.home/'.state'), CODEX_HOME=str(self.home/'.codex'), CLAUDE_CONFIG_DIR=str(self.home/'.claude'))
        self.env = patch.dict(os.environ, env, clear=True); self.env.start()
        self.spec = dict(agent='claude', remove=False, exe='/test/cairn', config=str(self.home/'.config/cairn/config.json'), instructions='memory instructions')
    def tearDown(self): self.env.stop(); self.temp.cleanup()
    def test_claude_preserves_other_hook_and_mcp_and_removes_own(self):
        folder=self.home/'.claude'; folder.mkdir()
        hook={'matcher':'Read','hooks':[{'type':'command','command':'my-helper'}]}
        (folder/'settings.json').write_text(json.dumps({'theme':'dark','hooks':{'PostToolUse':[hook]}}))
        (folder/'.claude.json').write_text(json.dumps({'mcpServers':{'other':{'command':'other'}}}))
        setup.configure(self.spec)
        before={p:setup.read(p) for p in folder.iterdir()}
        setup.configure(self.spec)
        self.assertEqual(before,{p:setup.read(p) for p in folder.iterdir()})
        self.spec['remove']=True; setup.configure(self.spec)
        self.assertEqual(json.loads(setup.read(folder/'settings.json')), {'theme':'dark','hooks':{'PostToolUse':[hook]}})
        self.assertEqual(json.loads(setup.read(folder/'.claude.json')), {'mcpServers':{'other':{'command':'other'}}})
    def test_conflict_keeps_every_agent_file_unchanged(self):
        folder=self.home/'.claude'; folder.mkdir()
        (folder/'settings.json').write_text('{"theme":"dark"}')
        (folder/'.claude.json').write_text('{"mcpServers":{"cairn":{"command":"other"}}}')
        before={p:setup.read(p) for p in folder.iterdir()}
        with self.assertRaisesRegex(ValueError,'unmarked'): setup.configure(self.spec)
        self.assertEqual(before,{p:setup.read(p) for p in folder.iterdir()})
    def test_codex_unmarked_name_conflict_keeps_hook_config_unchanged(self):
        self.spec['agent']='codex'
        folder=self.home/'.codex'; folder.mkdir()
        path=folder/'hooks.json'; path.write_text('{"description":"user hooks"}')
        existing=json.dumps([{'name':'cairn','transport':{'command':'user-server','env':{}}}])
        with patch.object(setup, 'native', return_value=existing), self.assertRaisesRegex(ValueError, 'unmarked'):
            setup.configure(self.spec)
        self.assertEqual(path.read_text(), '{"description":"user hooks"}')

    def test_opencode_unmarked_name_conflict_preserves_jsonc_and_instructions(self):
        self.spec['agent']='opencode'
        folder=self.home/'.config/opencode'; folder.mkdir(parents=True)
        path=folder/'opencode.jsonc'
        raw='{ /* user */ "mcp":{"servers":{"cairn":{"command":["user-server"]},}},}'
        path.write_text(raw)
        rules=folder/'AGENTS.md'; rules.write_text('User instructions\n')
        with patch('shutil.which', return_value='/stub/opencode'), self.assertRaisesRegex(ValueError, 'unmarked'):
            setup.configure(self.spec)
        self.assertEqual(path.read_text(), raw)
        self.assertEqual(rules.read_text(), 'User instructions\n')

    def test_native_codex_can_start_with_a_missing_config_directory(self):
        from types import SimpleNamespace
        def run(*args, **kwargs):
            self.assertTrue((self.home/'.codex').is_dir())
            return SimpleNamespace(returncode=0, stdout='[]')
        with patch('shutil.which', return_value='/stub/codex'), patch('subprocess.run', side_effect=run):
            self.assertEqual(setup.native('codex', ['list','--json'], self.home), '[]')

    def test_missing_native_cli_keeps_instruction_and_hook_files_absent(self):
        for agent in ['codex','opencode']:
            self.spec['agent']=agent
            with patch('shutil.which', return_value=None), self.assertRaisesRegex(ValueError,'installed'):
                setup.configure(self.spec)
        self.assertFalse((self.home/'.codex/hooks.json').exists())
        self.assertFalse((self.home/'.config/opencode/AGENTS.md').exists())

    def test_sessionstart_covers_compaction_without_a_precompact_hook(self):
        root = {}
        setup.hook_config(root, 'claude', '/hook.py', '/cfg.json', False)
        setup.hook_config(root, 'codex', '/hook.py', '/cfg.json', False)
        for agent_hooks in (root['hooks'],):
            self.assertIn('SessionStart', agent_hooks)
            self.assertNotIn('PreCompact', agent_hooks)
            self.assertNotIn('PostCompact', agent_hooks)
            self.assertNotIn('matcher', agent_hooks['SessionStart'][-1])

    def test_opencode_plugin_install_and_remove_keep_unrelated_config(self):
        self.spec['agent'] = 'opencode'
        secret = 'device-secret-value'
        config = Path(self.spec['config'])
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'url':'http://127.0.0.1:9','token':secret}))
        folder = self.home/'.config/opencode'
        folder.mkdir(parents=True)
        (folder/'plugin').mkdir()
        (folder/'plugin'/'other.js').write_text('keep-plugin\n')
        (folder/'plugin'/'cairn.js').write_text('// cairn:continuity\nlegacy\n')
        (folder/'plugin'/'stranger.js').write_text('not cairn\n')
        raw = '''{
 // keep
 "plugins": ["other-plugin", {"package":"kept","options":{"cairn":"continuity"}}, {"package":"/tmp/opencode/plugin/cairn.js","options":{"cairn":"continuity"}}],
 "mcp": {"servers": {"other": {"command":["keep"]}}}
}
'''
        path = folder/'opencode.jsonc'
        path.write_text(raw)
        (folder/'AGENTS.md').write_text('User instructions\n')
        with patch('shutil.which', return_value='/stub/opencode'), patch.object(setup, 'native', return_value=''):
            setup.configure(self.spec)
            text = path.read_text()
            plugin = (folder/'plugins'/'cairn'/'index.js').read_text()
            self.assertIn('// keep', text)
            self.assertIn('other-plugin', text)
            self.assertIn('kept', text)
            self.assertNotIn('plugin/cairn.js', text)
            hook = str(config.parent/'hook'/'cairn_hook.py')
            self.assertTrue(plugin.startswith('// cairn:continuity\n'))
            self.assertIn(hook, plugin)
            self.assertIn(str(config), plugin)
            self.assertIn('CAIRN_CONFIG', plugin)
            self.assertIn('session.hook', plugin)
            self.assertIn('compaction', plugin)
            self.assertIn('execute.after', plugin)
            self.assertNotIn(secret, text)
            self.assertNotIn(secret, plugin)
            self.assertFalse((folder/'plugin'/'cairn.js').exists())
            self.assertEqual((folder/'plugin'/'stranger.js').read_text(), 'not cairn\n')
            self.assertIn('keep-plugin', (folder/'plugin'/'other.js').read_text())
            self.assertIn('User instructions', (folder/'AGENTS.md').read_text())
            self.assertIn('cairn:begin', (folder/'AGENTS.md').read_text())
            again = path.read_text()
            setup.configure(self.spec)
            self.assertEqual(again, path.read_text())
            self.assertEqual(plugin, (folder/'plugins'/'cairn'/'index.js').read_text())
            (folder/'plugins'/'cairn'/'index.js').write_text('user plugin\n')
            setup.configure(self.spec)
            self.assertEqual((folder/'plugins'/'cairn'/'index.js').read_text(), 'user plugin\n')
            (folder/'plugins'/'cairn'/'index.js').write_text(plugin)
            self.spec['remove'] = True
            setup.configure(self.spec)
        removed = path.read_text()
        self.assertIn('// keep', removed)
        self.assertIn('other-plugin', removed)
        self.assertIn('kept', removed)
        self.assertNotIn('plugin/cairn.js', removed)
        self.assertNotIn(secret, removed)
        self.assertFalse((folder/'plugins'/'cairn'/'index.js').exists())
        self.assertEqual((folder/'plugin'/'other.js').read_text(), 'keep-plugin\n')
        self.assertEqual((folder/'plugin'/'stranger.js').read_text(), 'not cairn\n')
        self.assertEqual((folder/'AGENTS.md').read_text(), 'User instructions\n')
        self.assertIn('"other"', removed)

if __name__=='__main__': unittest.main()
