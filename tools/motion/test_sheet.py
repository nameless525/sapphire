"""Contact sheet orchestration, without cooking or starting an application."""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sheet


class Sheet(unittest.TestCase):
    def test_uses_the_cargo_reported_executable_and_keeps_generator_warnings_visible(self):
        artifact = {'reason': 'compiler-artifact', 'executable': '/custom-target/debug/sapphire_sandbox',
                    'target': {'name': 'sapphire_sandbox', 'kind': ['bin']}}
        completed = types.SimpleNamespace(stdout=json.dumps(artifact) + '\n')
        with tempfile.TemporaryDirectory() as out, patch.object(sheet, 'OUT', out), \
                patch.object(sheet.subprocess, 'run', return_value=completed) as run, \
                contextlib.redirect_stdout(io.StringIO()):
            sheet.main(['light_1:1,2', '--travel'])
        generator, content, build, capture = run.call_args_list
        self.assertNotIn('capture_output', generator.kwargs)
        self.assertIn('--message-format=json', build.args[0])
        self.assertEqual(capture.args[0][0], artifact['executable'])
        self.assertNotIn('--clip-in-place', capture.args[0])
        self.assertEqual(capture.kwargs['env']['SAPPHIRE_CONTENT_DIR'],
                         os.path.join(sheet.REPO, 'target/content/desktop/'))

    def test_shows_failed_subprocess_output(self):
        error = subprocess.CalledProcessError(2, ['cargo'], output='compiler detail\n', stderr='failure\n')
        with patch.object(sheet.subprocess, 'run', side_effect=error), \
                contextlib.redirect_stderr(io.StringIO()) as output, self.assertRaises(SystemExit):
            sheet.checked(['cargo'])
        self.assertEqual(output.getvalue(), 'compiler detail\nfailure\n')

    def test_unknown_flags_or_missing_values_fail_before_writing(self):
        for args in [['light_1', '--wriet'], ['light_1', '--views']]:
            with self.subTest(args=args), patch.object(sheet.subprocess, 'run') as run, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                sheet.main(args)
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
