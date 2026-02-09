#!/bin/bash
# Check if this is a jj repository with maw workspaces
if [ -d .jj ] || jj status &>/dev/null; then
  echo "IMPORTANT: This project uses Jujutsu (jj) for version control with GitHub for sharing. Use jj commands instead of git (e.g., \`jj status\`, \`jj describe\`, \`jj log\`). To push to GitHub, use bookmarks and \`jj bookmark set <name> -r @\` then \`jj git push\`."
  if [ -d .workspaces ] || maw ws list &>/dev/null 2>&1; then
    echo "This project uses maw for workspace management. Use \`maw ws create <name>\` to create isolated workspaces, \`maw ws merge <name> --destroy\` to merge back to main."
  fi
fi
exit 0
