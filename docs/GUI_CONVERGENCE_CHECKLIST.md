# GUI Convergence Checklist

Use this after applying all future bundles.

## Compile and repair
- [ ] `npm run build`
- [ ] fix first error only
- [ ] rebuild
- [ ] repeat until clean

## Shell validation
- [ ] app loads
- [ ] left rail renders all major screens
- [ ] global status bar renders
- [ ] desk mode toggle persists
- [ ] no full-page vertical scrolling on desktop

## Screen validation
- [ ] dashboard renders
- [ ] execution renders
- [ ] risk renders
- [ ] reconcile renders
- [ ] portfolio renders
- [ ] ops renders
- [ ] runtime renders
- [ ] metrics / transport / topology render
- [ ] alerts / incidents / audit / artifacts / strategy render

## Truth-model validation
- [ ] `dataSource` exists on `SystemModel`
- [ ] status bar shows source state
- [ ] `mockData.ts` includes `dataSource`
- [ ] fallback state is explicit
- [ ] disconnected state is visible

## Daemon agreement validation
- [ ] `/v1/status` or `/api/v1/system/status` resolves cleanly
- [ ] `/v1/trading/account` mapping does not break portfolio summary
- [ ] `/v1/trading/positions` mapping does not break positions
- [ ] `/v1/trading/orders` mapping does not break orders tables
- [ ] `/v1/trading/fills` mapping does not break fills tables

## Final hygiene
- [ ] remove stale backup imports if any remain
- [ ] commit as one GUI convergence checkpoint
