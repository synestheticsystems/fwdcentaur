"""What the wallboard's build (build-wallboard.sh) refuses before it writes anything."""
import os
from pathlib import Path
import shutil
import subprocess
import unittest

REPO = Path(__file__).resolve().parent.parent


class OfficialBuild(unittest.TestCase):
    def test_it_refuses_the_rustflags_cargo_would_take_in_place_of_the_remapping(self):
        bash = shutil.which('bash')
        for variable in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
            with self.subTest(variable):
                env = {name: value for name, value in os.environ.items()
                       if name not in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS')}
                # Set, though empty, as Cargo still takes it. The search path leaves trunk out, so that a
                # build that failed to refuse stops at its check for trunk instead of building.
                env.update({'WALLBOARD_REMAP_PATHS': '1', variable: '', 'PATH': '/usr/bin:/bin'})
                result = subprocess.run([bash, str(REPO / 'scripts/build-wallboard.sh')], cwd=REPO, env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(f'refuses {variable}:', result.stderr)


if __name__ == '__main__':
    unittest.main()
