# TUI Screenshot

When making visual changes to the TUI, update the README screenshot:

```bash
./scripts/screenshot-tui.sh           # Captures 1200x800 to images/tui.png
./scripts/screenshot-tui.sh 1600 900  # Custom dimensions
```

Requires: Hyprland, kitty, grim, pngquant. The script spawns a floating window, captures it, and compresses the image.
