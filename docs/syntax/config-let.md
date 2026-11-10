# `config let` (deprecated): see Settings

This page moved to [Settings: `let X = tunable(…)`](settings.md).

`config let X = v` declares a setting, and it still works, but it is
deprecated: write `let X = tunable(v)`, which means the same
([The deprecated `config let`](settings.md#the-deprecated-config-let)).
The compile check warns about each one, and `petal migrate-old-code` rewrites
it. This stub stays for one release
([plan](../tasks/lifecycle-annotation-functions-refactor.md)).
