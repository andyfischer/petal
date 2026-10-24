Seed programs for the direct-manipulation v2 speculative runner
(`core/tests/dm_v2_spec.rs`, `recompile_oracle_agrees`): each literal of each
file is overridden in turn, by slot and by recompiling the edited source, and
the two runs must emit the same values. They emit through `push_output`, so
they run in core with no host. M5 replaces them with
`examples/direct-manipulation-v2/`.
