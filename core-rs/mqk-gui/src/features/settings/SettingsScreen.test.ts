import assert from "node:assert/strict";
import React from "react";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { SettingsScreen } from "./SettingsScreen";
import { MOCK_MODEL } from "../system/mockData";

test("Settings / Operations renders session profile diagnostics", () => {
  const html = renderToStaticMarkup(React.createElement(SettingsScreen, { model: MOCK_MODEL }));

  assert.match(html, /Session profile/);
  assert.match(html, /Equity Us Regular/);
  assert.match(html, /Configured Override/);
  assert.match(
    html,
    /Equity regular session profile displayed through the existing always-on daemon calendar policy\./,
  );
  assert.match(html, /Crypto Continuous/);
  assert.match(html, /Futures Globex/);
  assert.match(html, /Forex 24x5/);
});
