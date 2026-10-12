# thumbs

File thumbnails the way the freedesktop desktops make them. A thumbnail
lives in the shared cache (`$XDG_CACHE_HOME/thumbnails`, the Thumbnail
Managing Standard), so one made by another file manager shows at once, and
the reverse. A missing one is made by a thumbnailer installed on the system
(a `.thumbnailer` entry, the contract GNOME, COSMIC and glycin share). It
runs as a plain argv in its own process group under a timeout, and its
output is checked before it is cached. No MixOS app decodes media itself.

The crate is headless: no toolkit, no pixels. Showing a cached thumbnail is
`toolkit::thumbnail`'s part.

- `Pool<K>` makes thumbnails off the caller's thread. Each `Request` has a
  `lane` (a new request replaces the one waiting in its lane) and a
  `Ticket` (a shared generation counter: bump it when the view moves on,
  and its requests are dropped or their runs killed). Replies go to the
  callback given to `Pool::new`, which also names the app the cache is
  written as.
- `generate::Generator` makes one thumbnail. Everything that touches the
  file before its thumbnailer runs happens on a probe thread under a
  deadline, so a hung filesystem strands a thread, never a worker.
- `mounts` refuses network, FUSE and automounted files from the mount table
  before anything touches them.
- `cache`, `png` and `registry` are the cache, the few PNG facts it needs,
  and the installed thumbnailers.

Known limits:

- Output is checked structurally, never decoded (no decoders here), so a
  well-formed PNG with corrupt image data is cached as ready. The display
  side shows the type icon for it until the file changes.
- There is no way to withdraw one lane's waiting request short of making
  its ticket stale. A folder view that scrolls will want one; it can be
  added without changing what exists.
- The PNG and mountinfo parsers have unit tests but no fuzz targets yet.

Users: dopus (the Properties preview). Build and test with
`cargo test -p thumbs`.
