# Rule packs

Diskray only ever cleans paths named by a rule. Rules live in TOML rule packs:

- `rules/core.toml` covers caches that common macOS apps and developer tools rebuild.
- `rules/apps/*.toml` and `rules/dev/*.toml` are bundled optional packs for
  specific apps (Telegram, TradingView, ZCode, Google, Adobe, AI coding tools,
  OrbStack, Playwright for Go).
- Your own packs go in `~/.config/diskray/rules/*.toml`.

All bundled packs are compiled into the binary and on by default. To turn one off,
list its `pack` name in `disabled_packs` in
`~/Library/Application Support/diskray/settings.json`:

```json
{ "online_research": false, "disabled_packs": ["telegram", "adobe"] }
```

`diskray rules list` shows every rule in effect and any warnings.
`diskray rules check my-pack.toml` validates a pack before you install it.

## Schema

```toml
schema = 1
pack = "notes"                      # lower-case letters, digits, dashes
description = "Caches from Example Notes."

[[rule]]
id = "cache"                        # full id is notes.cache
label = "Example Notes cache"       # shown in the interface, up to 60 characters
path = "Library/Caches/com.example.notes"   # relative to your home folder
tier = "routine"                    # routine | reinstallable | review
target = "contents"                 # contents (default) | aged
# min_age_days = 30                 # required for aged rules
processes = ["/Example Notes.app/"] # running apps that mark the data in use
associate = ["/example notes.app/"] # lower-case command fragments linking processes to this rule
note = "cached previews reload"     # the tradeoff shown to the user
quick_win = false                   # eligible for the Quick wins list
advice = "Prefer the app's own storage settings."

[rule.guidance]                     # optional detail shown in the inspector
classification = "APP CACHE"
delete_scope = "Cached previews inside this folder."
impact = "Previews reload on next open."
recovery = "Regenerated automatically."
recommendation = "Safe to clear while the app is closed."
```

### Tiers

| Tier | Behavior |
| --- | --- |
| `routine` | Eligible for ordinary cleanup once every safety check passes. |
| `reinstallable` | Shown but unavailable by default; Space opts in. Using the tool later may download again. |
| `review` | Never part of ordinary or unattended cleanup. Only a separate single-item `DELETE` plan can remove it. |

### Targets

- `contents` empties the folder and keeps the folder itself.
- `aged` removes only direct entries that, together with everything inside
  them, are untouched for `min_age_days` and not open by any process.

## Validation

A pack is rejected when it is not valid TOML, uses an unknown field, or has
`schema` other than 1. A single rule is rejected, with a reason, when:

- its `path` is absolute, starts with `~`, contains `.`, `..`, empty segments,
  glob characters, or control characters, or is longer than 512 bytes;
- its `path` is the same as, inside, or around another rule's path (bundled
  rules win; your rule is skipped with a warning);
- it is `review` tier and sets `quick_win` or `native`;
- an `aged` rule has no `min_age_days` (1–3650), or a `contents` rule sets one;
- a `processes` entry is longer than 200 characters or joins alternatives with a
  top-level `|` (write one entry per alternative; `(a|b)` groups are fine).

### Native commands

A bundled rule may set `native` to the id of a compiled-in vendor command
(`pip`, `go`, `uv`, `yarn`, `playwright`, `cypress`, `huggingface`). Each command
is pinned to one exact path, so a rule can only use it for that path. User packs
cannot run native commands at all; their rules always use the guarded
exact-path cleanup.

## Contributing a pack

1. Add `rules/apps/<app>.toml` or `rules/dev/<tool>.toml`.
2. Add it to `BUILTIN_PACKS` in `src/rules.rs`.
3. Run `cargo test` (every bundled pack must validate) and
   `diskray rules check rules/apps/<app>.toml`.
4. In the pull request, explain how you know the data is safe to remove, and
   which running processes use it.
