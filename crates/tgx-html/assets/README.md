# Where these files came from

`style.css`, `script.js` and `images/*.png` are copied **verbatim** from an
export produced by Telegram Desktop itself. They are the assets Desktop lays
beside its own `messages.html`, and they are shipped here so that an export from
this app is byte-for-byte the same document rather than a reproduction of one.

They were previously reimplemented by hand: 6.2 KB of CSS against Desktop's
43 KB, and no images at all, so every `media_file` / `media_photo` /
`media_voice_message` row rendered as a blank square. Reproducing a stylesheet
by eye cannot converge on an exact match; shipping the real one can.

**Licensing.** Telegram Desktop is distributed under the GPL v3, and these
files are part of it. If this app is ever redistributed, that has to be squared
away — either by honouring the GPL for the whole distribution, or by generating
the assets at run time from a Desktop installation the user already has, or by
going back to a reproduction and accepting that the HTML will no longer match
byte for byte.

Do not edit these files. The parity legs compare our output against a real
Desktop export, and a local edit here would show up as a difference in every
single exported page.
