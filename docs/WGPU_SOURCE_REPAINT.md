# Source repaint and bounded warmup

The native event loop receives egui repaint requests through a user-event proxy.
Immediate requests wake the window; delayed requests use the earliest deadline
and `WaitUntil`. When no work remains the loop returns to `Wait`, not polling.
Source folding therefore no longer depends on another mouse event to display
the updated rows. The same scheduler advances idle warmup batches.

Source warmup is restricted to the current viewport and one viewport on either
side, using the visible-row map. It never walks all raw lines in the document.
An unchanged viewport preserves progress; scrolling, folding or cache-key changes
update the work queue. Each idle batch handles at most 12 candidates and checks
the 750 microsecond budget between candidates, including cache hits and skips.

Individual egui layout calls cannot be interrupted. Idle warmup skips code lines
or collapsed prefixes longer than 2048 UTF-8 bytes. This bounds input size, **not
wall-clock duration**. Very long lines still require synchronous layout when
actually displayed, and a jump to an uncached area can still have cold-cache
cost. No source text is truncated or modified.

Collapsed rows cache their line number independently of raw code. Expanding and
collapsing replaces the affected interval including its header, preserving the
correct summary even when nested folds start on the same line.

## Verification

Automated tests cover immediate/delayed egui callbacks without pointer input,
deadline coalescing and idle waiting, bounded viewport queues, hidden/long-line
skips, and fold splices compared with a freshly rebuilt visible-row map.

For manual acceptance, open a large Modelica class in Source and check:

- Click a fold and stop moving the pointer: the result appears without another event.
- Drag the scrollbar into new regions, then revisit them after a brief idle period.
- Toggle nested folds repeatedly; header text and visible bodies agree.
- After settling and warmup, idle rendering does not run continuously.

Optional PowerShell profiling (both flags can be enabled together):

```powershell
$env:MODELICA_WGPU_PROFILE_SOURCE_SCROLL = '1'
$env:MODELICA_WGPU_PROFILE_SOURCE_FOLD = '1'
cargo run --release -p modelica-wgpu -- '<model-or-library-path>'
```

The fold flag now enables render-stage timing independently of the scroll flag.
Automated tests do not substitute for GUI feel or measured p95/p99 latency.
