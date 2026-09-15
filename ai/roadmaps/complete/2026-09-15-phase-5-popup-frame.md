# Popup frame and idle visibility

Status: Complete

Remove the rectangular native frame behind the transparent card stack, and
prevent an empty popup from remaining visible as a horizontal line.

Use a true Win32 popup style and remove native edge styles whenever winit
rebuilds them. Keep the existing no-activation and taskbar exclusions. Reconcile
visibility against the HWND, hide before any empty-state resize, and retain the
last useful size while hidden. Do not change card appearance or polling.

egui-winit 0.36.1 always enables the Windows undecorated shadow when decorations
are false; `has_shadow` only affects macOS. Start hidden with decorations true
and strip native styles before showing, avoiding winit's one-pixel non-client
shadow edge. DWM corner/border settings alone cannot remove that edge.

Verify with cargo test, cargo clippy --all-targets, and the offline demo on the
Windows desktop: transparent margins, multiple cards, idle, reappearance, focus,
and recovery from externally changed visibility/styles.

Milestones: [roadmap](2026-09-15-roadmap-phase-5-popup-frame.md).

Verified on Windows with the release demo: startup, two/three running cards,
completed cards, idle, reappearance, and forced HWND visibility/caption changes.
All sampled active states had zero non-background pixels in the top three window
rows; idle screenshots were clear. Foreground focus stayed unchanged. All 100
unit tests and clippy passed. The isolated popup commit also passed its 97 tests
and clippy without the pre-existing local restart changes. Installed the release build with a timestamped
backup of the previous executable and restarted the existing monitor.
