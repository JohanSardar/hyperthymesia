# Synthetic model recordings

All names, events, times, and responses are authored synthetic examples. No live
CLI calls or personal source data were used. Files are named by `Packet.input_digest`.
`crates/hx-interpret/tests/recorded.rs` defines the timelines and authored responses
for success, instruction-bearing page/car claim, unsupported completion,
overlapping task intervals, unknown evidence reference, and self-generated support.

Normal tests replay these files. Explicitly regenerate after intentional fixture
input changes with `HX_WRITE_SYNTHETIC_FIXTURES=1 cargo test -p hx-interpret` (using
the configured shared cargo target directory).
