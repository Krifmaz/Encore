# Encore – YouTube sign-in extension

Gives your Encore server a YouTube sign-in for its YouTube Music downloads. Two ways:

* **Hand over a YouTube sign-in** (recommended). The extension opens a new private (incognito) window at
  Google's sign-in. You sign in there; once YouTube Music shows you signed in, the extension sends that
  window's youtube.com / google.com cookies to Encore (after a 3-second countdown, or at once with
  **Send to Encore**) and closes the window **without signing out**. Closing a private window throws its
  cookies away in the browser, so from then on the session belongs to Encore alone: Encore refreshes it by
  itself every 4 hours (and when YouTube asks it to sign in), also while your computer is off. If Google
  ever ends it, Encore tells the admins: do the hand-off again.
* **Sync this browser's own sign-in** (optional, in the options). Sends this browser's YouTube cookies when
  the browser starts, 30 seconds after a sign-in cookie changes, and every 6 hours. Encore never refreshes
  these itself (that would sign this browser out), so they only stay fresh while the browser is used.
  A handed-over sign-in always wins over these.

Cookies go to your Encore server and nowhere else. The browser's name and version (its User-Agent) go with
them: Encore refreshes a handed-over sign-in as that browser, since Google ties a sign-in to the browser it
was made in.

## Install

1. In Encore: Settings → Apps & sources → YouTube sign-in → **Connect the browser extension**. Copy the
   token (it is shown once and can only upload YouTube cookies).
2. Download `Encore-youtube-extension-<version>.zip` from the Encore releases page and unzip it.
   * **Chrome, Edge, Brave**: open `chrome://extensions` (`edge://extensions`, `brave://extensions`),
     switch on Developer mode, **Load unpacked** and pick the `chrome` folder. Then open the extension's
     **Details** and switch on **Allow in Incognito** (the hand-off needs a private window).
   * **Firefox**: open `about:debugging#/runtime/this-firefox`, **Load Temporary Add-on** and pick
     `firefox/manifest.json` (temporary add-ons are removed when Firefox closes; for a permanent install
     use Firefox Developer Edition / Nightly with `xpinstall.signatures.required` off, or sign it on
     addons.mozilla.org as an unlisted add-on). In `about:addons` → Encore – YouTube sign-in, set
     **Run in Private Windows** to **Allow**.
3. The options page opens (or click the extension's button → Options): enter your Encore address (for
   example `https://music.example.com`) and the token, then **Save**. Allow the permissions it asks for.
4. Click the extension's button → **Hand over a YouTube sign-in**, sign in to Google in the private window,
   and let it count down. Settings → Apps & sources → YouTube sign-in in Encore then shows "Handed-over
   sign-in" as the source, with the browser it came from; "Everyday browser (extension)" is the optional
   sync below. Encore refreshes the handed-over sign-in every 4 hours and says so on the card; when Google
   answers a refresh without new cookies, the card says the sign-in was only checked.

Making a new token in Encore stops the old one; enter the new one in the options.
