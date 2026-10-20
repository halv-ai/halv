# Efficient command execution

Use this guidance only when the tool supports the requested behavior.
This guidance does not change tool permissions or repository requirements.

- After an empty poll, request up to 30 seconds on the next read-only poll, if supported.
- Keep stdin empty while waiting for output.
- Never send a command or nonempty stdin to simulate a wait.
- Send process input only when the user or process requires that input.
- Do not let longer polls delay cancellation.
- Respect tool errors, timeouts, permissions, and repository instructions.
- Before JVM builds, identify the project wrapper, build system, JDK version, and required toolchain versions.
- Use the project wrapper and preserve declared toolchain versions.
- Do not change tools or versions only to bypass build failures.
- Run the smallest relevant test target first.
- Run broader tests when change risk, coverage gaps, or project instructions require them.
- Never skip, weaken, or rewrite valid tests only to make them pass.
