# Renders assets/demo.gif. Re-record with: vhs assets/demo.tape
# Needs lazypost on PATH and network access to httpbin.org.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1280
Set Height 720
Set Padding 16
Set TypingSpeed 70ms
Set Theme "Catppuccin Mocha"

# A fresh collection seeded with the examples.
Hide
Type "export XDG_DATA_HOME=$(mktemp -d) LAZYPOST_CONFIG_DIR=$(mktemp -d) && clear"
Enter
Show

Type "lazypost"
Sleep 300ms
Enter
Sleep 1.5s

# Send the GET example and look at the response.
Ctrl+S
Sleep 2.5s
Type "4"
Sleep 600ms
Type "jjjjj"
Sleep 800ms
Type "L"
Sleep 1.2s
Type "H"
Sleep 600ms

# Open the POST example; Enter keeps focus in the collection.
Type "1"
Sleep 400ms
Type "j"
Sleep 400ms
Enter
Sleep 800ms
Type "3"
Sleep 400ms
Type "LL"
Sleep 1.2s

# Rewrite the URL with @ variable completion (INSERT turns the UI yellow).
Type "2"
Sleep 500ms
Type "c"
Sleep 400ms
Type "@ba"
Sleep 1.2s
Enter
Sleep 400ms
Type "/anything"
Sleep 500ms
Escape
Sleep 500ms
Escape
Sleep 400ms

# Add a header row in the table.
Type "3"
Sleep 300ms
Type "H"
Sleep 600ms
Type "o"
Sleep 300ms
Type "X-Demo"
Tab
Type "lazypost"
Sleep 400ms
Enter
Sleep 600ms

# Send it.
Ctrl+S
Sleep 3s
Type "4"
Sleep 400ms
Type "jjjjjjjj"
Sleep 1.5s

# Help, then quit without saving.
Type "?"
Sleep 2s
Escape
Sleep 400ms
Type ":q!"
Sleep 500ms
Enter
Sleep 500ms
