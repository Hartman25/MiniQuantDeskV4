# GUI Future Bundle Merge Order

Apply these bundles in this exact order:

1. `gui_future_bundle.zip`
2. `gui_future_bundle_phase2.zip`
3. `gui_future_bundle_phase3.zip`
4. `gui_future_bundle_phase4.zip`
5. `gui_future_bundle_phase5.zip`
6. `gui_future_bundle_phase6.zip`

## Why this order

- Phase 1 lays the shell, truth model, runtime screen, and status surface.
- Phase 2 upgrades the primary operator screens.
- Phase 3 aligns navigation and layout components.
- Phase 4 upgrades secondary surfaces.
- Phase 5 upgrades tertiary surfaces.
- Phase 6 adds convergence styling, merge instructions, and final prep docs.

## After applying

From `core-rs/mqk-gui` run:

```powershell
npm run build
```

Then fix compile drift one error at a time.

## Expected first-pass repair targets

- prop mismatches where an existing local component contract differs from the future bundle
- type mismatches where the repo has evolved since bundle creation
- missing imports for screens added later, especially `RuntimeScreen.tsx`
- CSS class names that exist in screen files but not yet in the local stylesheet
