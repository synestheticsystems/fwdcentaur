// Tells the runner that VS Code got as far as the suite. Before the suite, the test runner in VS
// Code's extension host loads this file (the `preload` of .vscode-test.mjs), which writes the file
// that $FWD_SUITE_STARTED names. When VS Code exits and that file is missing, no test has run, and
// the runner has scripts/test-extension-headless.sh launch the suite again: on CI, VS Code exited
// so when its window did not answer for 15 seconds while it loaded (task EyLMg55J). A launch that
// got this far is never launched again, so that no failing test runs a second time.

import * as fs from 'fs';

if (process.env.FWD_SUITE_STARTED) fs.writeFileSync(process.env.FWD_SUITE_STARTED, `${process.pid}\n`);
