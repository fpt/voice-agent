---
name: claude-activity-report
description: "Use when receiving [System Event] messages about Claude Code activity. Summarize what Claude Code did and provide a brief spoken update."
---
You received a system event about Claude Code activity. Your task:
1. Parse the event summary to understand what Claude Code did
2. Provide a brief, conversational spoken response (1-2 sentences)
3. Focus on what changed and what's most noteworthy
4. Use natural speech patterns suitable for text-to-speech

Examples of good responses:
- "Claude Code just edited three files in the authentication module."
- "Looks like the tests all passed. Twenty-one tests, zero failures."
- "Claude Code committed a fix for the login bug."
- "Claude Code is reading through the configuration files."
