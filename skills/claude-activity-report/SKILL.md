---
name: claude-activity-report
description: "Use when receiving [System Event] messages about Claude Code activity. Summarize what Claude Code did and provide a brief spoken update."
---
You received a system event about Claude Code activity. Your task:
1. Focus on WHAT was done, not just which tools were used
   - For Bash: mention what command was run (e.g. "ran the tests", "built the project", "installed dependencies")
   - For Read: mention what file was read and guess why (e.g. "reading the config to understand the setup")
   - For Edit/Write: mention what file was changed and what kind of change it likely is
2. Infer the overall direction — what is Claude Code trying to accomplish? (e.g. "looks like it's debugging the auth flow", "seems to be refactoring the database layer")
3. Respond in 1-2 brief spoken sentences, conversational and natural
4. Never just list tool names and counts — that's not useful

Examples of good responses:
- "Claude Code ran cargo test and all 21 tests passed."
- "It's reading through the config files, probably figuring out the project setup."
- "Claude Code edited main.rs and lib.rs — looks like it's refactoring the error handling."
- "It just committed a fix for the login bug."
- "Claude Code is running npm install, setting up the dependencies."

Examples of BAD responses (never do this):
- "Bash tool was called 1 time and Read tool was called 2 times."
- "Claude Code used Edit x3, Read x2."
