# Shared settings for every Diskray demo recording (https://github.com/charmbracelet/vhs).
Set Shell "bash"
Set FontSize 18
Set Width 1400
Set Height 820
Set Padding 24
Set Framerate 8
Set CursorBlink false
Set TypingSpeed 45ms
Set Theme { "name": "Diskray", "background": "#0f141d", "foreground": "#ebf0f7", "cursor": "#6fbeff", "black": "#151d29", "red": "#e68977", "green": "#a2c297", "yellow": "#e6b868", "blue": "#6fbeff", "magenta": "#c4a7ff", "cyan": "#98b7bc", "white": "#ebf0f7", "brightBlack": "#3a495d", "brightRed": "#e68977", "brightGreen": "#a2c297", "brightYellow": "#e6b868", "brightBlue": "#6fbeff", "brightMagenta": "#c4a7ff", "brightCyan": "#98b7bc", "brightWhite": "#ffffff", "selection": "#27394e" }
Hide
Type `unset NO_COLOR; export COLORTERM=truecolor PS1='$ ' PATH="$PWD/target/release:$PATH"; export DEMO=$(sh scripts/demo_fixture.sh "$(mktemp -d /tmp/diskray-demo.XXXXXX)"); clear; echo DEMO_READY`
Enter
Wait+Screen@30s /DEMO_READY/
Show
