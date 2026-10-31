# helm-service

`HelmService`: a Windows service (LocalSystem) that keeps the Helm agent
running in the active console session. MIT licensed, written from the
Microsoft Learn documentation.

- Finds the console session (`WTSGetActiveConsoleSessionId`), gets the
  signed-in user's token (`WTSQueryUserToken`) and environment
  (`CreateEnvironmentBlock`), and starts `helm-agent.exe` as that user on
  `winsta0\default` (`CreateProcessAsUserW`). The agent sits in a
  kill-on-close job object, so it never outlives the service.
- Restarts the agent when it exits: 1 s, 2 s, 4 s ... capped at 60 s; a run
  of 30 s or more resets the streak.
- Follows session changes (`SERVICE_CONTROL_SESSIONCHANGE`): fast user
  switching moves the agent to the new console session; logoff/logon and
  "no user signed in yet" are handled (polled every 10 s as a fallback).
- Logs to `%ProgramData%\Helm\service.log`.

```text
helm-service install [--agent <path>] [--start auto|demand] [-- <agent args>...]
helm-service uninstall | start | stop | status
helm-service run ...      (what the SCM invokes)
```

`--agent` defaults to `helm-agent.exe` next to `helm-service.exe`.
`install`/`uninstall`/`start`/`stop` need an elevated prompt. The installer
(`host/installer/helm.iss`) calls these for you.

The decision logic (`supervisor`, `backoff`, `cmdline`, `cli`) is pure and
covered by `cargo test`; the tests never install or start the service.

Not yet: running the agent on the secure desktop (UAC prompts, lock screen),
which is the next Phase 5 step.
