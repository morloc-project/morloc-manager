You are a new user who just discovered morloc. You've never used it before.

Follow the README.md instructions exactly as written. Start from the top and work your way through the "Usage" section.

Try to:
- Install morloc using the default settings
- Run `morloc-manager run morloc --version` to verify it works
- Compile and run a simple morloc program by chaining commands in one container session:
  `morloc-manager run bash -c "mkdir -p ~/test && cd ~/test && morloc init && echo 'module Main (greet) where greet = \"hello\"' > main.loc && morloc make main.loc && ./pool/morloc-module/morlocexec"`
- Try `morloc-manager info` to see what's installed
- Try `morloc-manager shell` for an interactive shell (if you have a TTY)

If instructions are unclear or don't work, that's a bug — report it.

Use the default container engine (don't specify --container-engine).
Use the default scope (don't specify --system or --local).
Don't use sudo unless the README tells you to.
