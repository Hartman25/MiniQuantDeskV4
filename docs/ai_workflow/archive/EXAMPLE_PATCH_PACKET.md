# Example Patch Packet — MiniQuantDesk V4

This is an example only. It is not a living source of truth.

## Patch ID
- **Patch ID:** GUI-TRUTH-EXAMPLE
- **Short title:** Distinguish authoritative empty from unavailable strategy summary truth

## Patch type
- truth-contract patch

## Controlling authority
- daemon contract for strategy summary truth
- GUI rendering files for the affected surface
- relevant GUI truth tests
- readiness rules requiring honest mounted truth

## Primary goal
Ensure the GUI does not present unavailable strategy-summary truth as if it were an authoritative empty result.

## Non-goals
- do not redesign daemon routes
- do not add real backend wiring
- do not broaden into unrelated GUI cleanup

## Strict scope

### Files to inspect first
- relevant GUI component for strategy summary rendering
- shared client/types used by that component
- daemon route contract or API type defining truth-state semantics
- relevant GUI truth test

### Files allowed only if strictly required
- adjacent helper or renderer
- shared status text helper

### Files out of scope
- unrelated daemon routes
- unrelated dashboard panels
- docs except direct contract clarification if unavoidable

## Current behavior / defect
The GUI collapses multiple states into one broad placeholder path, making unavailable or not-wired truth look equivalent to an authoritative empty result.

## Required outcome
The GUI must render distinct states for:
- authoritative empty
- unavailable / not wired
- present data

## Constraints
- do not widen scope
- do not redesign adjacent architecture
- preserve the daemon contract
- return full updated file(s) only

## Required proof / validation
- run the relevant GUI truth tests
- confirm the unavailable state and authoritative empty state render differently

## Success criteria
- unavailable truth is no longer displayed as a clean empty success state
- authoritative empty truth still renders honestly
- proof/test output confirms the change
