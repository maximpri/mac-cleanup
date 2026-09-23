# Demo recordings

Recorded with [vhs](https://github.com/charmbracelet/vhs) against the synthetic
home built by `scripts/demo_fixture.sh`: APFS clones that look like tens of
gigabytes but use almost no disk. Nothing in a recording touches real data;
`hero.tape` also runs with `--analyze`.

```bash
brew install vhs
sh scripts/build-release.sh
vhs docs/demo/why.tape      # docs/demo/why.gif
vhs docs/demo/hero.tape     # docs/demo/hero.gif, used at the top of the README
vhs docs/demo/ask.tape      # needs Apple Intelligence
vhs docs/demo/mcp.tape      # needs Claude Code; edits your user MCP config
```

After recording `hero.gif`, point the README image at it.
