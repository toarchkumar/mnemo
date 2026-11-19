# The terminal demo (v0200-d4): two tools share one encrypted brain.
#
# Re-record with the `demo` workflow (.github/workflows/demo.yml), which
# builds this commit's mnemo and runs this tape with VHS from the
# repository root; commit its docs/img/demo.gif (RELEASING.md). Every
# command runs for real against a throwaway brain (docs/demo/setup.sh).

Output docs/img/demo.gif

Require mnemo

Set Shell "bash"
Set FontSize 20
Set Width 1400
Set Height 540
Set Padding 28
Set TypingSpeed 28ms
Set Theme { "name": "MNemo", "black": "#151A1D", "red": "#F27A7A", "green": "#2BB3A6", "yellow": "#F0A35A", "blue": "#8AB4FF", "magenta": "#C4A7FF", "cyan": "#6FD6CB", "white": "#E8EAED", "brightBlack": "#848C96", "brightRed": "#F27A7A", "brightGreen": "#6FD6CB", "brightYellow": "#E8C37A", "brightBlue": "#8AB4FF", "brightMagenta": "#C4A7FF", "brightCyan": "#6FD6CB", "brightWhite": "#FFFFFF", "background": "#0E1113", "foreground": "#E8EAED", "selection": "#252C31", "cursor": "#2BB3A6" }

Hide
Type "source docs/demo/setup.sh"
Enter
# The model download can take a while on a fresh runner.
Wait@180s /\$\s*$/
Show

Type "# in Claude Code: save a decision, with a key"
Enter
Sleep 400ms
Type `mnemo remember team.mnemo --key decision.release --text "Ship from the release branch."`
Enter
Wait /\$\s*$/
Sleep 1.5s

Type "# in Cursor, later: recall it from the same file"
Enter
Sleep 400ms
Type `mnemo recall team.mnemo --text "how do we ship a release?" --top-k 1`
Enter
Wait /\$\s*$/
Sleep 2.5s

Type "# Cursor refines it; the new version replaces the old one"
Enter
Sleep 400ms
Type `mnemo remember team.mnemo --key decision.release --text "Ship from the release branch; tag on main."`
Enter
Wait /\$\s*$/
Sleep 1s
Type "mnemo history team.mnemo --key decision.release"
Enter
Wait /\$\s*$/
Sleep 2.5s

Type "# one encrypted file: what it holds, and no plain text"
Enter
Sleep 400ms
Type "mnemo about team.mnemo | head -n 1"
Enter
Wait /\$\s*$/
Sleep 1.5s
Type `grep -c "release branch" team.mnemo`
Enter
Wait /\$\s*$/
Sleep 4s
