# Encore – YouTube sign-in extension

Keeps Encore's YouTube Music downloads signed in with the YouTube account of your own browser. The
extension reads this browser's youtube.com and google.com cookies and sends them to your Encore server
(and nowhere else): when it is installed, when the browser starts, 30 seconds after a sign-in cookie
changes, and every 6 hours. Encore never refreshes these cookies itself, so your browser stays signed in.

## Install

1. In Encore: Settings → Apps & sources → YouTube sign-in → **Connect browser extension**. Copy the token
   (it is shown once and can only upload YouTube cookies).
2. Download `Encore-youtube-extension-<version>.zip` from the Encore releases page and unzip it.
   * **Chrome, Edge, Brave**: open `chrome://extensions`, switch on Developer mode, **Load unpacked** and
     pick the `chrome` folder.
   * **Firefox**: open `about:debugging#/runtime/this-firefox`, **Load Temporary Add-on** and pick
     `firefox/manifest.json` (temporary add-ons are removed when Firefox closes; for a permanent install
     use Firefox Developer Edition / Nightly with `xpinstall.signatures.required` off, or sign it on
     addons.mozilla.org as an unlisted add-on).
3. The options page opens (or click the extension's button): enter your Encore address (for example
   `https://music.example.com`) and the token, then **Save and send now**. Allow the permissions it asks for.
4. Stay signed in to YouTube in that browser. Settings → YouTube sign-in in Encore shows
   "Browser extension" as the source in use.

Making a new token in Encore stops the old one; enter the new one in the options.
