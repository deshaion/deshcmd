import json
import os
import pathlib
import subprocess
import tempfile
import unittest

import yaml


class ReleaseWorkflowTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        root = pathlib.Path(__file__).resolve().parents[2]
        cls.workflow = yaml.load(
            (root / ".github/workflows/build-packages.yml").read_text(),
            Loader=yaml.BaseLoader,
        )

    def test_only_promotes_after_signed_assets_are_uploaded(self):
        self.assertEqual(set(self.workflow["on"]), {"workflow_dispatch"})
        attach = self.workflow["jobs"]["attach"]
        self.assertIn("prepare", attach["needs"])
        steps = attach["steps"]
        signed_upload = next(
            index for index, step in enumerate(steps)
            if "dist/latest.json dist/latest.json.sig" in step.get("run", "")
        )
        promotion = next(
            index for index, step in enumerate(steps)
            if "--draft=false" in step.get("run", "")
        )
        self.assertLess(signed_upload, promotion)
        self.assertEqual(steps[signed_upload]["if"], "needs.prepare.outputs.signed == 'true'")
        self.assertNotIn("continue-on-error", steps[signed_upload])
        self.assertNotIn("if", steps[promotion])
        self.assertEqual(steps[promotion]["env"]["PROMOTE"], "${{ needs.prepare.outputs.signed }}")
        self.assertIn(' --latest="$PROMOTE" ', steps[promotion]["run"])

    def test_checks_draft_and_signing_status(self):
        preflight = next(
            step["run"] for step in self.workflow["jobs"]["prepare"]["steps"]
            if step.get("id") == "draft"
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            gh = root / "gh"
            gh.write_text('#!/bin/sh\nprintf "%s\\n" "$RELEASE_JSON"\n')
            gh.chmod(0o700)
            output = root / "output"
            for draft, prerelease, key, signed in [
                (False, False, "key", None),
                (True, True, "key", "false"),
                (True, False, "", "false"),
                (True, False, "key", "true"),
            ]:
                with self.subTest(draft=draft, prerelease=prerelease, key=key):
                    output.write_text("")
                    env = dict(
                        os.environ,
                        PATH=f"{root}:{os.environ['PATH']}",
                        GITHUB_REPOSITORY="test/deshcmd",
                        RELEASE_TAG="v0.1.1",
                        UPDATE_PUBLIC_KEY=key,
                        RELEASE_JSON=json.dumps({"isDraft": draft, "isPrerelease": prerelease}),
                        GITHUB_OUTPUT=str(output),
                    )
                    result = subprocess.run(
                        ["bash", "-eo", "pipefail", "-c", preflight],
                        env=env, capture_output=True, text=True,
                    )
                    if signed is None:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn("Release must be a draft", result.stderr)
                        self.assertEqual(output.read_text(), "")
                    else:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(output.read_text(), f"signed={signed}\n")
