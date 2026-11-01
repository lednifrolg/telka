# The README's demo, recorded with vhs (https://github.com/charmbracelet/vhs) against the demo
# TV in crates/telka/examples, so no request reaches a real TV. From the repository root:
#
#   cargo build -p telka --bin telka --examples && vhs assets/demo.tape

Output "assets/demo.gif"

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 14
Set Width 1200
Set Height 660
Set Padding 16
Set Theme "TokyoNight"
Set TypingSpeed 60ms
Set Framerate 30

Hide
Type "export TELKA_HOME=$(mktemp -d) TELKA_TV_URL=ws://127.0.0.1:18893 PATH=$PWD/target/debug:$PATH"
Enter
Type "target/debug/examples/demo_tv 18893 $TELKA_HOME > /dev/null & sleep 1"
Enter
Type "PS1='\[\e[1;34m\]❯\[\e[0m\] '; clear"
Enter
Show

Type "telka status"
Enter
Sleep 2.5s
Type "telka vol 18"
Enter
Sleep 2s
Type "telka"
Enter
Sleep 2s

# The remote: the d-pad and OK, then the volume.
Right
Sleep 400ms
Enter
Sleep 1.2s
Type "++"
Sleep 1.5s

# The picture: OLED light up two steps, then a picture mode.
Type "]"
Sleep 300ms
Type "]"
Sleep 1.5s
Type "v"
Sleep 600ms
Type "cin"
Sleep 800ms
Enter
Sleep 1.5s

# An app; its address field takes the focus, and the line types into it.
Type "a"
Sleep 700ms
Type "brow"
Sleep 900ms
Enter
Sleep 1.6s
Type "example.com"
Sleep 1.2s
Enter

# Someone on the sofa turns the volume with the remote.
Sleep 5s

Type "?"
Sleep 3.5s
Type "?"
Sleep 600ms
Type "q"
Sleep 500ms

Hide
Type "kill %1"
Enter
