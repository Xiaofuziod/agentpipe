# VHS tape for the README hero GIF. Render with:  vhs demo/agentpipe.tape
# Uses the demo stub binaries (no API calls) so the cross-vendor review loop
# runs deterministically: Claude fixes -> Codex reviews -> loop until clean.
Output demo/agentpipe.gif

Set Shell bash
Set FontSize 15
Set Width 1180
Set Height 860
Set Padding 22
Set TypingSpeed 22ms
Set Theme { "name": "AgentPipe", "background": "#080b0a", "foreground": "#e6edf0", "black": "#080b0a", "green": "#2ef08f", "brightGreen": "#46ffa6", "white": "#e6edf0", "cursor": "#2ef08f", "selection": "#1ea968" }

# --- hidden setup: point AgentPipe at the demo stubs, prep a throwaway repo ---
Hide
# run `vhs demo/agentpipe.tape` from the repo root
Type "export PATH=$PWD/target/release:$PATH" Enter
Type "export AGENTPIPE_CLAUDE_BIN=$PWD/demo/stub-claude.sh AGENTPIPE_CODEX_BIN=$PWD/demo/stub-codex.sh" Enter
Type "export AGENTPIPE_HOME=/tmp/ap-demo" Enter
Type "rm -f /tmp/ap-demo/codex.count; mkdir -p /tmp/ap-demo/repo; (cd /tmp/ap-demo/repo && git init -q)" Enter
Type "clear" Enter
Show

Sleep 600ms
Type "agentpipe run demo/demo-task.yaml" Sleep 700ms Enter
Sleep 11s
Sleep 2500ms
