# Real Ratatui session: folders, build output, commands, and read-only plan.
Output docs/demo/hero.gif
Source docs/demo/common.tape
Hide
Type "diskray --analyze --volume $DEMO"
Enter
Wait+Screen@60s /Scan.*files/
Type "b"
Wait+Screen@30s /CONTENTS HEATMAP/
Sleep 2s
Show
Screenshot docs/images/tui-storage.png
Sleep 3s
# Browse the measured Library folder and its developer data.
Enter
Wait+Screen@15s /Developer/
Sleep 2s
Enter
Wait+Screen@15s /CoreSimulator/
Sleep 3s
# Back to the root list, then open code/api/target/debug.
Escape
Sleep 500ms
Escape
Sleep 500ms
Down 2
Sleep 1s
Enter
Wait+Screen@15s /old-landing/
Sleep 1s
Enter
Wait+Screen@15s /Cargo.toml/
Sleep 1s
Enter
Wait+Screen@15s /debug/
Sleep 2s
Screenshot docs/images/tui-build-output.png
Enter
Wait+Screen@15s /blob-0/
Sleep 2s
Type ":"
Wait+Screen@15s /COMMANDS/
Sleep 2s
Escape
Sleep 500ms
Type "p"
Wait+Screen@15s /YOUR PLAN IS EMPTY/
Sleep 2s
Escape
Sleep 500ms
Type "b"
Sleep 1s
Hide
Type "q"
Sleep 500ms
