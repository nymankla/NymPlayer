---
paths:
  - "src/record.rs"
---

# Recording internals

- `record::Recording::start` creates the file up front (a bad folder fails immediately), then a writer thread drains the ring buffer (2 s) every 20 ms. The audio callback never touches the disk. The input is written **untouched**: no gain or mute, per the issue.
- Formats: 16-bit WAV via `hound`; 16-bit FLAC via `flacenc`, encoded **frame by frame** (`encode_fixed_size_frame`, 4096-frame blocks) so memory stays flat. The STREAMINFO header is written first and rewritten with the final counts on finish. The short last block is padded to FLAC's 32-frame minimum.
- Input gaps over `record::GAP` (100 ms) are recorded as silence, since loopback sends nothing while the PC is quiet.
- Dropping `Recording` stops it, drains the buffer and finishes the file (joins the thread). A write error also finishes the file; `ended()` gives the reason for the status line.
