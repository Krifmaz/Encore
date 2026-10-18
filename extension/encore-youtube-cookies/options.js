// The browser's extension settings open this page. Everything lives in the toolbar popup's page (popup.html,
// popup.js): it is shown here, in this tab, with ?tab for the wider layout, so there is one copy of it.
location.replace(`popup.html?tab${location.hash}`);
