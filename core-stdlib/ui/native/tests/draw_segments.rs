//! The segmented drain (`petal_ui::draw_segments`): a replayed helper's
//! commands come from the host's cache instead of being decoded again, and
//! the frames are exactly what the flat drain gives with memo off.
//!
//! Drives raw `FrameCore`s only. The corpus-wide oracle is
//! `segmented_draw_buffers_reproduce_unmemoized_frames_across_the_corpus`
//! in test/ui-corpus/tests/memo.rs.

use petal::env::Env;
use petal::policy::RunPolicy;
use petal_ui::draw::{DrawOptions, FrameCommand, register_draw_with, take_frame_commands_snapped};
use petal_ui::draw_segments::DrawSegments;
use petal_ui::frame_core::{FrameCore, FrameRun};
use petal_ui::subpixel::PixelSnap;

// Helpers that only draw, so memo records them: `grid` changes the snap
// mode inside itself, `row` re-records on the frames its input changes,
// and a cached layer and top-level draws sit between them.
const SRC: &str = "state n = 0
n += 1
fn grid(k)
  pixel_snap(true)
  for i in range(0, 20) do draw_rect({x: i * 1.5, y: k * 2.5, w: 1.25, h: 1.25}, :102030) end
  pixel_snap(nil)
  nil
end
fn row(k, t)
  for i in range(0, 12) do draw_circle({x: i * 3.5, y: k + t}, 1.5, :ff0000) end
  nil
end
fn panel(k)
  grid(k)
  row(k, 0.5)
  nil
end
panel(1)
grid(2)
cached_layer(\"k\", rect(3, 4, 50, 50), fn() draw_circle({x: 10.5, y: 7.25}, 2.5, :00ff00) end)
row(2, float(n % 3))
draw_rect({x: 0.5, y: 0.5, w: 2.5, h: 2.5}, :010203)
";

fn core(policy: RunPolicy) -> FrameCore {
    let mut env = Env::new();
    petal_ui::input::register_input(&mut env);
    register_draw_with(&mut env, DrawOptions { subpixel: true });
    petal_ui::register_prelude(&mut env);
    let script = petal_ui::script_for(&env, SRC);
    let program = env.load_program(&script).unwrap_or_else(|e| panic!("{e}"));
    let stack = env.create_stack(program).unwrap();
    let mut core = FrameCore::new(env, program, stack);
    core.set_policy(policy);
    core
}

fn run(core: &mut FrameCore, frame: i64) {
    core.invalidate();
    assert_eq!(core.frame(0.016, frame, &mut ()).unwrap(), FrameRun::Ran);
}

#[test]
fn the_segment_cache_gives_the_unmemoized_frames_and_decodes_only_what_is_new() {
    let mut flat = core(RunPolicy::REPLAY.with_memo(false));
    let mut seg = core(RunPolicy::REPLAY);
    let mut segs = DrawSegments::new();
    segs.enable(&mut seg.env);
    let mut total = 0;
    for frame in 1..=6 {
        run(&mut flat, frame);
        run(&mut seg, frame);
        let want: Vec<(FrameCommand, PixelSnap)> = take_frame_commands_snapped(&mut flat.env);
        let got = segs.take_frame_commands_snapped(&mut seg.env);
        assert_eq!(got, want, "frame {frame}");
        let stats = segs.stats();
        assert!(!stats.broken, "frame {frame}");
        total = want.len();
        if frame == 1 {
            assert_eq!(stats.reused, 0);
            assert!(stats.defined > 0);
        } else {
            // From frame 2 on the helpers replay: most of the frame comes
            // from the cache, and only `row(2, …)`'s new recording (when its
            // input moved) and the top-level draws are decoded.
            assert!(stats.reused >= 20 * 2 + 12, "frame {frame}: {stats:?}");
            assert!(stats.decoded < stats.reused, "frame {frame}: {stats:?}");
            assert!(stats.spliced > 0);
        }
    }
    assert!(total > 60);

    // A host that lost its cache starts over: the next drain defines what
    // it refers to again, and the frame is still the same.
    segs.reset(&mut seg.env);
    run(&mut flat, 7);
    run(&mut seg, 7);
    assert_eq!(segs.take_frame_commands_snapped(&mut seg.env), take_frame_commands_snapped(&mut flat.env));
    assert!(segs.stats().defined > 0 && !segs.stats().broken);

    // Off again: a flat drain, the same frames.
    segs.disable(&mut seg.env);
    run(&mut flat, 8);
    run(&mut seg, 8);
    assert_eq!(segs.take_frame_commands_snapped(&mut seg.env), take_frame_commands_snapped(&mut flat.env));
    assert_eq!(segs.stats().reused, 0);
}

/// The whole-pixel drain through the cache is the whole-pixel flat drain.
#[test]
fn the_segment_cache_drains_whole_pixels_like_take_draw_commands() {
    let mut flat = core(RunPolicy::REPLAY.with_memo(false));
    let mut seg = core(RunPolicy::REPLAY);
    let mut segs = DrawSegments::new();
    segs.enable(&mut seg.env);
    for frame in 1..=3 {
        run(&mut flat, frame);
        run(&mut seg, frame);
        assert_eq!(
            segs.take_draw_commands(&mut seg.env),
            petal_ui::draw::take_draw_commands(&mut flat.env),
            "frame {frame}"
        );
    }
    assert!(segs.stats().reused > 0);
}
