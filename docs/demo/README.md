# Demo recordings

The README's [color GIF](hero.gif) and PNG screenshots are captured from the
running Diskray binary in a terminal using
[VHS](https://github.com/charmbracelet/vhs). They are not UI mockups or rendered
test buffers. The app uses its normal Ratatui colors. The shared tape unsets
`NO_COLOR` and enables `COLORTERM=truecolor` in the recording shell.

`hero.tape` browses folders, opens `code/api/target/debug`, shows the command
palette, and reviews an empty plan. It runs with `--analyze`; cleanup and moves
are disabled. Screen-content waits check that each expected view appears before
the recording continues.

The sample home comes from `scripts/demo_fixture.sh`. APFS clones report about
23 GiB of allocated files while sharing a 256 MiB seed's physical storage.
Each run creates a fresh `/tmp/diskray-demo.*` directory. The recording uses
sample file names, but disk capacity, free space, process activity, and AI
availability are live readings from the recording Mac. These values vary;
inspect recordings before sharing them. The app may save scan history in its
normal application state directory.

## Reproduce the README media

Run from the repository root on macOS with an APFS temporary directory:

```bash
brew install vhs
sh scripts/build-release.sh
vhs validate 'docs/demo/*.tape'
vhs docs/demo/hero.tape
```

This writes:

- `docs/demo/hero.gif` — the README animation, 1400 × 820, about 23 seconds.
- `docs/images/tui-storage.png` — the folder browser and contents heatmap.
- `docs/images/tui-build-output.png` — measured Rust debug build output.

VHS records the real terminal screen, including waits and key navigation. The
opening scan is hidden so the animation starts with a populated view. No files
are cleaned or moved in the tour. The empty plan demonstrates the read-only
session, not an executed cleanup. Remove only the generated fixture directory
for your recording when finished; it is shown in the folder breadcrumb.

The still images provide a non-animated alternative. Review them and the entire
GIF after regenerating; check that text fits, colors survive encoding, and no
personal paths or process arguments appear.

## Other demo tapes

```bash
vhs docs/demo/why.tape      # docs/demo/why.gif
vhs docs/demo/ask.tape      # needs Apple Intelligence
vhs docs/demo/mcp.tape      # needs Claude Code; edits your user MCP config
```

The MCP tape is separate from the README recording and changes Claude Code's
user MCP configuration; inspect it before running it.
