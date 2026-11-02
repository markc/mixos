# VuIO wire oracle

Sanitised protocol captures from the local VuIO comparison on 2026-11-01.
These are observed wire data, not copied server source. Addresses use RFC 5737;
identity, friendly name and media names are neutral replacements. Media sizes,
object ids, XML structure and protocol headers are retained. The MP4 entry
keeps its extension and MIME type. No media content is included.

`oracle_test.mix` compares fields and comments/asserts deliberate differences:
path ids and URLs; flat root; extension-free titles; byte-only OP=01; no
Content-Disposition; local branding/routes; dlna rather than unused pv
namespace; live mtime update ids; added CDS queries; Server/Connection headers
and response-time Date. Whitespace and XML declaration case are immaterial.
