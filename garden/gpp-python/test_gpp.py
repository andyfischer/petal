#!/usr/bin/env python3
"""Unit tests for the `gpp` package — the protocol loop over in-memory streams,
the provider's public dispatch, replies/cache policies, decorators, background
handlers and the test harness.

Run:  python3 garden/gpp-python/test_gpp.py     (also on system python 3.9)
Mirrors petal-query's serve_core + provider tests: handshake, protocol
mismatch, query answers by id with JSON args and cache policies,
error/loading replies, mutations, navigation, emit dispatch, and the
forward-compatibility rules.
"""

import io
import json
import os
import threading
import unittest

from gpp import (
    AppError,
    BackgroundQuery,
    CachePolicy,
    Ctx,
    ErrorCode,
    Init,
    PanelUi,
    Provider,
    Reply,
    ScriptSink,
    TestHarness,
    background,
    script_args,
    serve_on,
)
from gpp.testing import by_id, init_req, notif, req, run


def init_ctx(arg="", **kw):
    """A Ctx for a direct provider dispatch (no host, no session)."""
    return Ctx(Init({"args": ["/repo"], "cwd": "/repo"}), arg, **kw)


class SessionRunTests(unittest.TestCase):
    def test_the_library_harness_replaces_the_hand_rolled_runner(self):
        # `run` is the helper every app's tests used to copy; it now ships.
        msgs = run(Provider(), PanelUi("d", "S"), [init_req(), notif("shutdown", {})])
        self.assertEqual(msgs[0]["result"]["protocol"], 2)


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


class ReplyTests(unittest.TestCase):
    def test_max_age_keeps_a_stale_window_set_earlier(self):
        # The documented bug: .stale_while_revalidate(...) then .max_age(...)
        # used to throw the window away because max_age built a fresh policy.
        wire = Reply.json(1).stale_while_revalidate(60).max_age(3)._policy.to_wire()
        self.assertEqual(wire, {"maxAgeMs": 3000, "staleWhileRevalidateMs": 60000})
        # …and the other order agrees.
        other = Reply.json(1).max_age(3).stale_while_revalidate(60)._policy.to_wire()
        self.assertEqual(other, wire)

    def test_forever_is_the_default_and_is_statable(self):
        self.assertIsNone(Reply.json(1)._policy.to_wire())
        self.assertIsNone(Reply.json(1).max_age(3).forever()._policy.to_wire())

    def test_no_store_replaces_the_whole_policy(self):
        self.assertEqual(Reply.json(1).max_age(3).no_store()._policy.to_wire(), {"noStore": True})

    def test_from_result_maps_values_exceptions_and_callables(self):
        self.assertEqual(Reply.from_result({"x": 1}).into_parts()[0], {"x": 1})
        self.assertEqual(Reply.from_result(AppError("nope")).into_parts()[1], "nope")
        self.assertEqual(Reply.from_result(lambda: 7).into_parts()[0], 7)
        # A callable that raises becomes the error, so a handler can say
        # `return Reply.from_result(lambda: risky())`.
        failed = Reply.from_result(lambda: 1 / 0)
        self.assertTrue(failed.is_error())
        self.assertIn("ZeroDivisionError", failed.into_parts()[1])
        # An AppError from a callable keeps its bare message (no type prefix).
        def refuse():
            raise AppError("not a git repo")
        self.assertEqual(Reply.from_result(refuse).into_parts()[1], "not a git repo")
        # A Reply passes through untouched, policy included.
        passthrough = Reply.from_result(Reply.json(1).max_age(2))
        self.assertEqual(passthrough._policy.to_wire(), {"maxAgeMs": 2000})

    def test_into_parts_splits_the_three_outcomes(self):
        self.assertEqual(Reply.json(1).into_parts()[:2], (1, None))
        self.assertEqual(Reply.error("e").into_parts()[:2], (None, "e"))
        self.assertEqual(Reply.loading().into_parts()[:2], (None, None))
        self.assertTrue(Reply.loading().is_loading())


class ProviderDispatchTests(unittest.TestCase):
    """The public dispatch surface: one call per handler, no stdio session."""

    def test_build_then_answer_dispatches_like_the_rust_provider(self):
        provider = Provider(lambda init: init.repo_arg()).query(
            "log", lambda repo, ctx: Reply.json({"repo": repo, "kind": ctx.kind}))
        init = Init({"args": ["/repo"], "cwd": "/repo"})
        state = provider.build(init)
        reply = provider.answer(state, Ctx(init, "", kind="log"))
        self.assertEqual(reply.into_parts()[0], {"repo": "/repo", "kind": "log"})

    def test_answer_wraps_plain_values_and_catches_handler_bugs(self):
        provider = Provider().query("n", lambda s, c: {"x": 1}).query("bug", lambda s, c: 1 / 0)
        self.assertEqual(provider.answer(None, init_ctx(kind="n")).into_parts()[0], {"x": 1})
        self.assertIn("ZeroDivisionError",
                      provider.answer(None, init_ctx(kind="bug")).into_parts()[1])

    def test_answer_of_an_unregistered_kind_is_a_null_value(self):
        self.assertEqual(Provider().answer(None, init_ctx(kind="nope")).into_parts()[:2],
                         (None, None))

    def test_mutate_dispatches_and_reports_unknown_names(self):
        provider = Provider(lambda i: {"n": 0})
        provider.on_mutation("bump", lambda s, c: Reply.json(s["n"] + c.arg))
        state = {"n": 1}
        self.assertEqual(provider.mutate(state, init_ctx(2, name="bump")).into_parts()[0], 3)
        self.assertIn("no mutation handler",
                      provider.mutate(state, init_ctx(name="nope")).into_parts()[1])
        self.assertTrue(provider.has_mutation("bump"))
        self.assertFalse(provider.has_mutation("nope"))

    def test_handle_emit_is_fire_and_forget_and_swallows_bugs(self):
        seen = []
        provider = Provider().on_emit("ui", lambda s, c: seen.append(c.arg))
        provider.on_emit("bug", lambda s, c: 1 / 0)
        provider.handle_emit(None, init_ctx({"x": 1}, event="ui"))
        provider.handle_emit(None, init_ctx(None, event="nope"))  # no handler: no-op
        provider.handle_emit(None, init_ctx(None, event="bug"))   # a bug must not raise
        self.assertEqual(seen, [{"x": 1}])
        self.assertTrue(provider.has_emit("ui"))

    def test_navigate_dispatch_returns_the_source_or_none(self):
        provider = Provider()
        self.assertFalse(provider.has_navigate())
        # No custom handler: None, so the caller falls back to declared screens.
        self.assertIsNone(provider.navigate(None, init_ctx(screen="x.ptl")))
        provider.on_navigate(lambda s, c: "// %s" % c.screen)
        self.assertTrue(provider.has_navigate())
        self.assertEqual(provider.navigate(None, init_ctx(screen="x.ptl")), "// x.ptl")

    def test_navigate_refusal_raises_app_error(self):
        def refuse(s, ctx):
            raise AppError("no such screen '%s'" % ctx.screen)
        provider = Provider().on_navigate(refuse)
        with self.assertRaises(AppError):
            provider.navigate(None, init_ctx(screen="x.ptl"))

    def test_has_query_reports_registrations(self):
        provider = Provider().query("a", lambda s, c: 1)
        self.assertTrue(provider.has_query("a"))
        self.assertFalse(provider.has_query("b"))

    def test_stateless_builds_none(self):
        self.assertIsNone(Provider.stateless().build(Init({})))


class DecoratorTests(unittest.TestCase):
    def test_every_registration_has_a_decorator_form(self):
        provider = Provider(lambda init: {"seen": []})

        @provider.query("log")
        def log(state, ctx):
            return Reply.json("log:%s" % ctx.arg_str())

        @provider.mutation("apply")
        def apply_(state, ctx):
            return Reply.json("applied %s" % ctx.arg)

        @provider.emit("divider")
        def divider(state, ctx):
            state["seen"].append(ctx.arg)

        @provider.navigate
        def nav(state, ctx):
            return "// %s" % ctx.screen

        # The decorators give the plain function back, so the module name
        # still refers to something callable and testable on its own.
        self.assertTrue(callable(log) and callable(nav))

        h = TestHarness(provider)
        self.assertEqual(h.query("log", "x").value(), "log:x")
        self.assertEqual(h.mutate("apply", 3).value(), "applied 3")
        self.assertEqual(h.navigate("detail.ptl").source(), "// detail.ptl")
        state = {"seen": []}
        provider.handle_emit(state, init_ctx(9, event="divider"))
        self.assertEqual(state["seen"], [9])

    def test_the_positional_form_stays_fluent(self):
        provider = (
            Provider()
            .query("a", lambda s, c: 1)
            .on_mutation("m", lambda s, c: Reply.json(2))
            .on_emit("e", lambda s, c: None)
            .on_navigate(lambda s, c: "src")
        )
        self.assertIsInstance(provider, Provider)
        self.assertTrue(provider.has_query("a") and provider.has_mutation("m"))
        self.assertTrue(provider.has_emit("e") and provider.has_navigate())

    def test_host_owned_mutation_names_are_refused(self):
        for name in ["open_path", "open_project", "open_pr", "open_file_dialog"]:
            with self.assertRaises(ValueError, msg=name) as caught:
                Provider().on_mutation(name, lambda s, c: Reply.json(1))
            self.assertIn("host-owned", str(caught.exception))
        # The decorator form refuses at decoration time too.
        with self.assertRaises(ValueError):
            @Provider().mutation("open_path")
            def never(state, ctx):
                return Reply.json(1)


class BackgroundTests(unittest.TestCase):
    def test_a_slow_handler_answers_loading_then_invalidates_then_serves(self):
        release = threading.Event()
        calls = []

        def slow(state, ctx):
            calls.append(ctx.arg_str())
            release.wait(5.0)
            return Reply.json({"rows": 3}).max_age(30.0)

        handler = background(slow)
        provider = Provider().query("stats", handler)
        out = io.StringIO()
        sink = ScriptSink(out)
        ctx = Ctx(Init({"cwd": "/repo"}), "k", kind="stats", sink=sink)

        # 1. The first ask returns at once, while the work is still blocked.
        first = provider.answer(None, ctx)
        self.assertTrue(first.is_loading())
        # 2. A re-ask while in flight coalesces: still loading, still one job.
        self.assertTrue(provider.answer(None, ctx).is_loading())
        self.assertEqual(handler.pending(), 1)

        release.set()
        self.assertTrue(handler.wait(5.0))
        self.assertEqual(calls, ["k"], "the two asks coalesced onto one run")

        # 3. The worker pushed an invalidate for that exact key.
        pushes = [json.loads(l) for l in out.getvalue().splitlines()]
        self.assertEqual(pushes, [{"jsonrpc": "2.0", "method": "invalidate",
                                   "params": {"kind": "stats", "arg": "k"}}])

        # 4. The re-query the invalidate triggers gets the real answer, with
        #    the handler's own cache policy.
        landed = provider.answer(None, ctx)
        self.assertEqual(landed.into_parts()[0], {"rows": 3})
        self.assertEqual(landed._policy.to_wire(), {"maxAgeMs": 30000})

    def test_background_keys_by_kind_and_arg(self):
        handler = background(lambda s, ctx: Reply.json(ctx.arg["page"]))
        provider = Provider().query("rows", handler)
        init = Init({})
        a = Ctx(init, {"page": 1}, kind="rows")
        b = Ctx(init, {"page": 2}, kind="rows")
        provider.answer(None, a)
        provider.answer(None, b)
        self.assertTrue(handler.wait(5.0))
        self.assertEqual(provider.answer(None, a).into_parts()[0], 1)
        self.assertEqual(provider.answer(None, b).into_parts()[0], 2)

    def test_a_failing_background_handler_lands_as_an_error(self):
        def boom(state, ctx):
            raise AppError("upstream is down")

        handler = background(boom)
        provider = Provider().query("x", handler)
        ctx = init_ctx(kind="x")
        self.assertTrue(provider.answer(None, ctx).is_loading())
        self.assertTrue(handler.wait(5.0))
        self.assertEqual(provider.answer(None, ctx).into_parts()[1], "upstream is down")

    def test_the_decorator_form_registers_a_background_handler(self):
        provider = Provider()

        @provider.background_query("slow")
        def slow(state, ctx):
            return Reply.json("done")

        self.assertIsInstance(provider._queries["slow"], BackgroundQuery)
        ctx = init_ctx(kind="slow")
        self.assertTrue(provider.answer(None, ctx).is_loading())
        provider._queries["slow"].wait(5.0)
        self.assertEqual(provider.answer(None, ctx).into_parts()[0], "done")

    def test_the_serve_loop_never_blocks_on_a_background_handler(self):
        # The whole point: a slow query must not hold the pipe, so the second
        # request in the same session is answered while the work runs.
        release = threading.Event()
        handler = background(lambda s, c: (release.wait(5.0), Reply.json("late"))[1])
        provider = Provider().query("slow", handler).query("fast", lambda s, c: Reply.json("now"))
        msgs = run(provider, PanelUi("d", "S"), [
            init_req(),
            req(11, "query", {"kind": "slow", "arg": ""}),
            req(12, "query", {"kind": "fast", "arg": ""}),
            notif("shutdown", {}),
        ])
        self.assertEqual(by_id(msgs, 11)["result"], {}, "loading, not a stall")
        self.assertEqual(by_id(msgs, 12)["result"]["value"], "now")
        release.set()
        handler.wait(5.0)


class SinkOpenPathTests(unittest.TestCase):
    def test_open_path_emits_the_reserved_event_with_an_absolute_path(self):
        out = io.StringIO()
        ScriptSink(out).open_path("relative/file.txt")
        env = json.loads(out.getvalue())
        self.assertEqual(env["method"], "emit")
        self.assertEqual(env["params"]["event"], "open_path")
        path = env["params"]["arg"]["path"]
        self.assertTrue(os.path.isabs(path), path)
        self.assertTrue(path.endswith("relative/file.txt"))


class HarnessTests(unittest.TestCase):
    def test_the_harness_drives_a_whole_session_per_call(self):
        provider = Provider(lambda init: init.repo_arg())
        provider.query("repo", lambda repo, ctx: Reply.json(repo).max_age(5.0))
        provider.query("boom", lambda s, ctx: Reply.error("nope"))
        provider.query("slow", lambda s, ctx: Reply.loading())
        ui = PanelUi("app", "SRC").screen("detail.ptl", "DETAIL")
        h = TestHarness(provider, ui, args=["/somewhere"], cwd="/somewhere")

        self.assertEqual(h.query("repo").value(), "/somewhere")
        self.assertEqual(h.query("repo").cache(), {"maxAgeMs": 5000})
        self.assertTrue(h.query("slow").is_loading())

        failed = h.query("boom")
        self.assertFalse(failed.ok())
        self.assertEqual(failed.error_message(), "nope")
        self.assertEqual(failed.error_code(), ErrorCode.APP)
        with self.assertRaises(AssertionError):
            failed.value()
        with self.assertRaises(AssertionError):
            h.query("repo").error_message()

        self.assertEqual(h.navigate("detail.ptl", {"id": 7}).source(), "DETAIL")
        self.assertIn("no such screen", h.navigate("nope.ptl").error_message())

        # The handshake and the pushes are inspectable too.
        head = h.handshake()
        self.assertEqual(head[0]["result"]["name"], "app")
        self.assertEqual(head[1]["params"]["source"], "SRC")
        self.assertEqual(h.query("repo").pushed("setScript")[0]["params"]["source"], "SRC")

    def test_the_harness_needs_no_panel_ui(self):
        provider = Provider().query("x", lambda s, c: 1)
        self.assertEqual(TestHarness(provider).query("x").value(), 1)

    def test_send_runs_several_requests_in_one_session(self):
        provider = Provider(lambda init: {"n": 0})
        provider.on_mutation("bump", lambda s, c: Reply.json(s.__setitem__("n", s["n"] + 1) or s["n"]))
        h = TestHarness(provider)
        msgs = h.send(req(11, "mutate", {"name": "bump", "arg": None}),
                      req(12, "mutate", {"name": "bump", "arg": None}))
        self.assertEqual(by_id(msgs, 12)["result"]["value"], 2, "one state across the session")

    def test_emit_returns_the_session_output(self):
        seen = []
        provider = Provider().on_emit("e", lambda s, c: seen.append(c.arg))
        msgs = TestHarness(provider).emit("e", {"x": 1})
        self.assertEqual(seen, [{"x": 1}])
        self.assertEqual(len(msgs), 2, "an emit is answered by nothing")


class CtxTests(unittest.TestCase):
    def test_arg_str_and_label_describe_the_dispatch(self):
        self.assertEqual(init_ctx({"a": 1}, kind="k").arg_str(), "")
        self.assertEqual(init_ctx("s", kind="k").arg_str(), "s")
        self.assertEqual(init_ctx(kind="k").label(), "query 'k'")
        self.assertEqual(init_ctx(name="m").label(), "mutation 'm'")
        self.assertEqual(init_ctx(event="e").label(), "emit 'e'")
        self.assertEqual(init_ctx(screen="s").label(), "navigate 's'")

    def test_the_serve_loop_hands_handlers_the_sink(self):
        got = {}

        def handler(state, ctx):
            got["sink"] = ctx.sink
            ctx.sink.status("working")
            return Reply.json(1)

        msgs = run(Provider().query("q", handler), PanelUi("d", "S"),
                   [init_req(), req(4, "query", {"kind": "q", "arg": ""})])
        self.assertIsInstance(got["sink"], ScriptSink)
        statuses = [m for m in msgs if m.get("method") == "emit"]
        self.assertEqual(statuses[0]["params"]["arg"], {"text": "working"})


if __name__ == "__main__":
    unittest.main()
