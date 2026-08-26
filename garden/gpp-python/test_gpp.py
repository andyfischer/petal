#!/usr/bin/env python3
"""Unit tests for gpp.py — the protocol loop over in-memory streams.

Run:  python3 garden/gpp-python/test_gpp.py
Mirrors petal-query's serve_core tests: handshake, protocol mismatch, query
answers by id with JSON args and cache policies, error/loading replies,
mutations, navigation, emit dispatch, and the forward-compatibility rules.
"""

import io
import json
import unittest

from gpp import (
    AppError,
    CachePolicy,
    ErrorCode,
    Init,
    PanelUi,
    Provider,
    Reply,
    ScriptSink,
    script_args,
    serve_on,
)


def init_req(protocol=2, args=None, cwd="/repo"):
    return {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocol": protocol,
            "paneId": 0,
            "rows": 40,
            "cols": 120,
            "args": args if args is not None else ["/repo"],
            "cwd": cwd,
            "capabilities": ["query", "mutate", "navigate", "emit", "hotReload"],
        },
    }


def req(req_id, method, params):
    return {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}


def notif(method, params):
    return {"jsonrpc": "2.0", "method": method, "params": params}


def run(provider, ui, messages):
    """Feed `messages` (dicts) to serve_on; return the output envelopes."""
    lines = "".join(json.dumps(m) + "\n" for m in messages)
    out = io.StringIO()
    serve_on(provider, ui, io.StringIO(lines), out)
    return [json.loads(line) for line in out.getvalue().splitlines()]


def by_id(msgs, req_id):
    for m in msgs:
        if m.get("id") == req_id and "method" not in m:
            return m
    raise AssertionError(f"no response with id {req_id} in {msgs}")


class HandshakeTests(unittest.TestCase):
    def test_handshake_reports_protocol_2_and_pushes_the_script(self):
        msgs = run(Provider(), PanelUi("demo", "SCRIPT"), [init_req(), notif("shutdown", {})])
        result = msgs[0]["result"]
        self.assertEqual(result["protocol"], 2)
        self.assertEqual(result["name"], "demo")
        self.assertIn("query", result["capabilities"])
        self.assertEqual(msgs[1]["method"], "setScript")
        self.assertEqual(msgs[1]["params"]["source"], "SCRIPT")
        self.assertNotIn("id", msgs[1], "setScript is a notification")

    def test_protocol_mismatch_is_refused_with_a_clean_error(self):
        # A pre-versioning host: no `protocol` field decodes as 1.
        old = init_req()
        del old["params"]["protocol"]
        msgs = run(Provider(), PanelUi("demo", "S"), [old])
        self.assertEqual(len(msgs), 1, "no script push after a refused handshake")
        err = msgs[0]["error"]
        self.assertEqual(err["code"], ErrorCode.PROTOCOL_MISMATCH)
        self.assertIn("protocol 2", err["message"])

    def test_title_from_state_names_the_pane(self):
        provider = Provider(lambda init: init.repo_arg())
        ui = PanelUi("static", "S", title=lambda repo: f"repo: {repo}")
        msgs = run(provider, ui, [init_req(), notif("shutdown", {})])
        self.assertEqual(msgs[0]["result"]["name"], "repo: /repo")

    def test_script_args_strips_the_python_script_itself(self):
        # `python3 app.py /repo --dev` — the host's args mirror the argv, so
        # the script's own path arrives as args[0] and must be strippable.
        init = Init({"args": ["/apps/my/app.py", "/repo", "--dev"], "cwd": "/x"})
        self.assertEqual(script_args(init, "/apps/my/app.py"), ["/repo", "--dev"])
        self.assertEqual(
            [a for a in script_args(init, "/apps/my/app.py") if not a.startswith("-")],
            ["/repo"],
        )

    def test_repo_arg_falls_back_to_cwd(self):
        seen = {}
        provider = Provider(lambda init: seen.setdefault("repo", init.repo_arg()))
        run(provider, PanelUi("d", "S"), [init_req(args=[], cwd="/somewhere")])
        self.assertEqual(seen["repo"], "/somewhere")


class QueryTests(unittest.TestCase):
    def test_queries_answer_by_id_with_json_args_and_policies(self):
        provider = (
            Provider(lambda init: init.repo_arg())
            .query("log", lambda repo, ctx: Reply.json({"repo": repo}).cache(
                CachePolicy.max_age(3).stale_while_revalidate(60)))
            .query("table", lambda repo, ctx: Reply.json({"page": ctx.arg["page"]}))
        )
        msgs = run(provider, PanelUi("git", "S"), [
            init_req(),
            req(5, "query", {"kind": "log", "arg": ""}),
            req(6, "query", {"kind": "table", "arg": {"name": "users", "page": 2}}),
            notif("shutdown", {}),
        ])
        log = by_id(msgs, 5)["result"]
        self.assertEqual(log["value"]["repo"], "/repo")
        self.assertEqual(log["cache"], {"maxAgeMs": 3000, "staleWhileRevalidateMs": 60000})
        # No kind/arg echo anywhere — the id is the correlation.
        self.assertNotIn("kind", log)
        self.assertNotIn("arg", log)
        table = by_id(msgs, 6)["result"]
        self.assertEqual(table["value"]["page"], 2)
        self.assertNotIn("cache", table, "a forever policy adds nothing to the wire")

    def test_a_plain_return_value_is_wrapped_as_a_json_reply(self):
        provider = Provider().query("n", lambda s, ctx: {"x": 1})
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "n", "arg": ""}), notif("shutdown", {})])
        self.assertEqual(by_id(msgs, 4)["result"]["value"], {"x": 1})

    def test_a_failed_query_is_an_error_response(self):
        provider = Provider().query("boom", lambda s, ctx: Reply.error("upstream failed"))
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "boom", "arg": ""}), notif("shutdown", {})])
        resp = by_id(msgs, 4)
        self.assertNotIn("result", resp)
        self.assertEqual(resp["error"], {"code": ErrorCode.APP, "message": "upstream failed"})

    def test_a_raised_app_error_is_an_error_response(self):
        def handler(s, ctx):
            raise AppError("not a git repo: /tmp")
        provider = Provider().query("log", handler)
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "log", "arg": ""})])
        self.assertEqual(by_id(msgs, 4)["error"]["message"], "not a git repo: /tmp")

    def test_an_unexpected_exception_is_an_app_error_not_a_crash(self):
        provider = Provider().query("bug", lambda s, ctx: 1 / 0)
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(),
            req(4, "query", {"kind": "bug", "arg": ""}),
            req(5, "query", {"kind": "bug", "arg": ""}),  # the loop kept serving
        ])
        err = by_id(msgs, 4)["error"]
        self.assertEqual(err["code"], ErrorCode.APP)
        self.assertIn("ZeroDivisionError", err["message"])
        self.assertIn("error", by_id(msgs, 5))

    def test_a_loading_reply_is_an_empty_result(self):
        provider = Provider().query("slow", lambda s, ctx: Reply.loading())
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "slow", "arg": ""})])
        self.assertEqual(by_id(msgs, 4)["result"], {})

    def test_an_unregistered_kind_answers_null(self):
        msgs = run(Provider(), PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "nope", "arg": ""})])
        result = by_id(msgs, 4)["result"]
        self.assertIn("value", result)
        self.assertIsNone(result["value"])

    def test_no_store_crosses_the_wire(self):
        provider = Provider().query("live", lambda s, ctx: Reply.json(1).no_store())
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "query", {"kind": "live", "arg": ""})])
        self.assertEqual(by_id(msgs, 4)["result"]["cache"], {"noStore": True})


class MutateTests(unittest.TestCase):
    def test_mutation_dispatches_and_returns_a_value(self):
        state = {"total": 0}

        def bump(s, ctx):
            s["total"] += ctx.arg["by"]
            return Reply.json({"total": s["total"]})

        provider = Provider(lambda init: state).on_mutation("bump", bump)
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(9, "mutate", {"name": "bump", "arg": {"by": 5}})])
        self.assertEqual(by_id(msgs, 9)["result"]["value"]["total"], 5)
        self.assertEqual(state["total"], 5)

    def test_an_unregistered_mutation_is_an_error(self):
        msgs = run(Provider(), PanelUi("d", "S"), [
            init_req(), req(9, "mutate", {"name": "nope", "arg": None})])
        self.assertIn("no mutation handler", by_id(msgs, 9)["error"]["message"])


class NavigateTests(unittest.TestCase):
    def test_navigate_serves_a_declared_screen_and_refuses_others(self):
        ui = PanelUi("d", "HOME").screen("detail.ptl", "DETAIL")
        msgs = run(Provider(), ui, [
            init_req(),
            req(4, "navigate", {"screen": "detail.ptl", "arg": {"id": 7}}),
            req(5, "navigate", {"screen": "nope.ptl"}),
        ])
        ok = by_id(msgs, 4)["result"]
        self.assertEqual(ok, {"screen": "detail.ptl", "source": "DETAIL"})
        err = by_id(msgs, 5)["error"]
        self.assertEqual(err["code"], ErrorCode.APP)
        self.assertEqual(err["message"], "no such screen 'nope.ptl'")

    def test_a_custom_navigate_handler_wins(self):
        visits = []

        def nav(s, ctx):
            visits.append((ctx.screen, ctx.arg))
            return f"// source for {ctx.screen}"

        provider = Provider().on_navigate(nav)
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(), req(4, "navigate", {"screen": "x.ptl", "arg": {"id": 7}})])
        self.assertEqual(by_id(msgs, 4)["result"]["source"], "// source for x.ptl")
        self.assertEqual(visits, [("x.ptl", {"id": 7})])


class EmitAndCompatTests(unittest.TestCase):
    def test_emit_reaches_its_handler_and_unknown_events_are_skipped(self):
        seen = []
        provider = Provider(lambda init: None).on_emit(
            "ui_state", lambda s, ctx: seen.append(ctx.arg))
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(),
            notif("emit", {"event": "ui_state", "arg": {"left_frac": 300}}),
            notif("emit", {"event": "mystery", "arg": None}),
        ])
        self.assertEqual(seen, [{"left_frac": 300}])
        # Notifications are never answered.
        self.assertEqual(len(msgs), 2, "only the handshake response + setScript")

    def test_an_unknown_request_gets_method_not_found(self):
        msgs = run(Provider(), PanelUi("d", "S"), [
            init_req(), req(7, "frobnicate", {})])
        err = by_id(msgs, 7)["error"]
        self.assertEqual(err["code"], ErrorCode.METHOD_NOT_FOUND)

    def test_an_unknown_notification_is_ignored(self):
        msgs = run(Provider(), PanelUi("d", "S"), [
            init_req(), notif("frobnicate", {}), notif("shutdown", {})])
        self.assertEqual(len(msgs), 2)

    def test_shutdown_ends_the_session(self):
        msgs = run(Provider().query("q", lambda s, c: Reply.json(1)), PanelUi("d", "S"), [
            init_req(),
            notif("shutdown", {}),
            req(4, "query", {"kind": "q", "arg": ""}),  # never reached
        ])
        self.assertEqual(len(msgs), 2)


class SinkTests(unittest.TestCase):
    def test_sink_writes_compact_single_lines(self):
        out = io.StringIO()
        sink = ScriptSink(out)
        sink.set_script("a\nb")  # embedded newline must be escaped, not raw
        sink.invalidate("log", "")
        sink.status("3 files")
        lines = out.getvalue().splitlines()
        self.assertEqual(len(lines), 3)
        self.assertEqual(json.loads(lines[0])["params"]["source"], "a\nb")
        self.assertEqual(json.loads(lines[1]),
                         {"jsonrpc": "2.0", "method": "invalidate",
                          "params": {"kind": "log", "arg": ""}})
        self.assertEqual(json.loads(lines[2])["params"],
                         {"event": "status", "arg": {"text": "3 files"}})


if __name__ == "__main__":
    unittest.main()
