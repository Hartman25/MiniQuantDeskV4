# Third-Party Notices

This directory (`.claude/skills/`) adapts procedural ideas from the upstream
source below. Adaptation is idea/structure-level (phase names, question
lists); MQD-specific content (proof classes, broker/DB invariants, authority
rules) is original to this repository.

## mattpocock/skills

- Repository: https://github.com/mattpocock/skills
- Pinned commit: `3cca18b368ae95cdbdebbff572ccafa662551015`
- License: MIT

Upstream copyright notice, reproduced verbatim per the MIT license terms:

```
MIT License

Copyright (c) 2026 Matt Pocock

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Adapted MQD files

- `.claude/skills/mqd-diagnose/SKILL.md` — adapted from `skills/engineering/diagnosing-bugs/SKILL.md`
- `.claude/skills/mqd-test-proof/SKILL.md` — informed by the evidentiary rigor in `skills/engineering/code-review/SKILL.md` and `skills/engineering/diagnosing-bugs/SKILL.md`; the question set itself is MQD-original (audit/proof rules for this repo)
- `.claude/skills/mqd-review-patch/SKILL.md` — adapted from `skills/engineering/code-review/SKILL.md` (multi-axis, read-only review structure)
- `.claude/skills/mqd-handoff/SKILL.md` — adapted from `skills/productivity/handoff/SKILL.md`
- `.claude/skills/mqd-external-research/SKILL.md` — adapted from `skills/engineering/research/SKILL.md`

Do not update this file to track a newer upstream commit without a new
explicit mission authorizing re-pinning.
