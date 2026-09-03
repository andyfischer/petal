# Hopper

A small side-scroller written to be **manipulated while it runs**. Run,
jump, collect eight coins, reach the flag. It is deliberately tiny (one file,
no randomness, no level loader) so that every tool in the repo can reach all
of it: hot reload, the direct-manipulation tracer, the agent protocol, and the
time-travel timeline it was built to exercise.

```bash
./examples/games/hopper/run.sh            # windowed
./examples/games/hopper/run.sh --headless # agent protocol on stdin
```

The launcher builds `petal-sdl` on first use.

## Controls

| Key | Action |
|---|---|
| `←` `→` / `A` `D` | Run |
| `space` / `↑` / `W` | Jump — tap for a hop, hold for a leap |
| `R` | Restart |

And the host's time-travel keys, which work on any script:

| Key | Action |
|---|---|
| `F5` | Freeze / resume. Resuming from an earlier frame rewinds the game to it |
| `,` `.` | While frozen: scrub back / forward. Hold for real time, `shift` for 5× |
| `F6` | Toggle the trail overlay |
| `F7` | Track the shape under the pointer through every recorded frame |

## The demo

1. Play up to the first jump and press `F5` mid-air.
2. Hold `,` to rewind to just before takeoff. Hover the player and press `F7`.
   The blue path is where the player has been; the orange path is where the
   recorded input takes them next.
3. In the editor, change `GRAVITY` (line 19) to `700.0` and save. The frames
   after the cursor are replayed through the new code with the same recorded
   input, and the orange arc rises. Change `JUMP_VY`, save again, and it
   changes again — unless the cursor is already past takeoff, in which case
   nothing moves, because the impulse already happened. Both answers are
   right, and the picture says which.
4. Press `F5`. The game resumes from the frozen moment, under the edited
   physics, with the score and everything else exactly as it was.

Every knob at the top of `game.ptl` is a `config let`, which is also what
Garden's drag-to-edit prefers to rewrite.

## From an agent

The same loop over the [agent protocol](../../../integrations/petal-desktop-sdl/docs/agent-protocol.md#timeline):

```
→ {"cmd":"input","keys_down":["right"]}      → {"cmd":"step","n":40}
→ {"cmd":"input","keys_down":["right","space"]}  → {"cmd":"step","n":15}
→ {"cmd":"input","keys_down":["right"]}      → {"cmd":"step","n":45}
→ {"cmd":"freeze"}                           → {"cmd":"scrub","to":-58}
→ {"cmd":"track","x":265,"y":477}            ← {"site":{"callee":"draw_rect","line":238,...}}
→ {"cmd":"trail"}                            ← 100 points, the jump arc as numbers
   (edit GRAVITY in game.ptl; the watcher reloads and replays 58 frames)
→ {"cmd":"trail"}                            ← the new arc
→ {"cmd":"screenshot"}                       ← the frozen frame with both trails drawn
→ {"cmd":"unfreeze"}                         → {"cmd":"state"}
```

`rewind` (`{"cmd":"rewind","frames":60}`) restores the state from a minute
of frames ago in one step; `state` afterwards reports the earlier values
exactly, which is a convenient way for an agent to explore alternatives from
a common starting point.

## Files

```
hopper/
  game.ptl     the whole game: knobs, level data, physics, drawing
  run.sh       launcher (extra arguments go to petal-sdl)
  README.md
  DEBRIEF.md   what worked, what did not, and where this should go
```
