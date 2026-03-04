GUI Hot Restart Scaffold

Added:
- gui/src/lib/runtimeApi.ts
- gui/src/components/RuntimeControlPanel.tsx

Manual merge suggestion:
- Add <RuntimeControlPanel baseUrl="http://localhost:8080" /> into an admin/settings page.
- Do NOT expose restart without auth.
