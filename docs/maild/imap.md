# IMAP protocol support

maild serves IMAP over implicit TLS. Sessions start in IMAP4rev1 compatibility
mode; `ENABLE IMAP4rev2` switches to rev2 responses and UTF-8 mailbox names.
`ENABLE UTF8=ACCEPT` enables UTF-8 while retaining rev1 SEARCH responses.
Rev1 mailbox names use modified UTF-7, including `&-` for a literal ampersand.
Non-canonical shifts, including shifted printable ASCII, fail with BAD. CREATE,
RENAME and subscriptions share Net-Unicode character validation, including C1
control rejection. Explicit `CHARSET US-ASCII` and `CHARSET UTF-8` work in every
SEARCH mode.

## Commands and extensions

| Surface | Behaviour | Wire coverage in `services/maild/tests/imap_rev2.rs` |
|---|---|---|
| MIME FETCH | Stored encoded content, nested part paths, MIME and forwarded-message selectors, PEEK and octet partials; numbering follows BODYSTRUCTURE | `thunderbird_mime_parts_on_demand_and_structure_agree` |
| FETCH structures | BODY omits extension fields; FAST, ALL and FULL expand to their standard data items | `fetch_macros_and_non_extensible_body` |
| BINARY | Transfer decoding, decoded SIZE and partials; literal8 for NUL; UNKNOWN-CTE on decoding failures | `binary_decodes_sizes_partials_literal8_and_seen`, `binary_unknown_and_malformed_transfer_encoding_fail` |
| ENABLE | IMAP4rev2 and UTF8=ACCEPT; unknown capabilities ignored | `enable_rev2_esearch_save_and_dollar_survive_expunge`, `utf8_mailbox_names_and_rev1_modified_utf7_roundtrip` |
| ESEARCH / SEARCHRES | MIN, MAX, ALL, COUNT, SAVE and `$`; saved identities remain UIDs across expunges | `enable_rev2_esearch_save_and_dollar_survive_expunge`, `legacy_search_and_save_min_max_with_uid_copy_and_store` |
| STATUS | SIZE, DELETED and configured APPENDLIMIT, alongside existing counters and rev1 RECENT | `status_aggregates_and_appendlimit_match_config` |
| LIST | SUBSCRIBED, REMOTE, RECURSIVEMATCH, CHILDREN, SPECIAL-USE, multiple patterns and LIST-STATUS; LIST and LSUB reference names | `list_extended_references_patterns_status_and_special_use` |
| CREATE-SPECIAL-USE | CREATE USE assigns one supported role to the leaf, with atomic uniqueness enforcement | Existing `imap_phase2` CREATE USE tests |
| Literals | Bounded segmented framing for command arguments, literal8 and MULTIAPPEND; pre-auth resource limits remain stricter | `general_literals_login_names_flags_search_and_multiappend`, `utf8_append_wrapper_and_literal_kind_are_checked` |
| Atomic APPEND | All message rows, projections, memberships and UID allocations commit together | `multiappend_failure_leaves_no_partial_messages`, `multiappend_storage_failure_rolls_back_all_rows_and_uids` |
| RENAME INBOX | Move current messages to a new mailbox; keep INBOX identity, UIDNEXT and children | `rename_inbox_preserves_identity_children_and_moves_messages` |
| UIDPLUS / COPY / MOVE | APPENDUID and COPYUID; atomic COPY; MOVE reports COPYUID before EXPUNGE | `copy_failure_rolls_back_earlier_destinations_and_uidnext`, existing `imap_phase4_copy_move_expunge` tests |
| Read-only and CLOSE | EXAMINE never sets Seen; CLOSE quietly expunges only a writable mailbox; UNSELECT retains deleted messages | `examine_fetch_and_close_unselect_have_distinct_effects` |
| Notifications | Preserve client MSNs until expunges can be reported; announce observed additions; IDLE reports flags with UIDs | `pending_expunge_preserves_msns_until_a_permitted_command`, existing `imap_phase5_idle` tests |
| Subscriptions | Names survive RENAME and DELETE; LIST SUBSCRIBED identifies absent mailboxes | `subscriptions_survive_rename_and_delete_and_absent_names_can_be_removed` |
| Shutdown | Finish the current response, send BYE, flush and release connection slots | `shutdown_sends_bye_during_idle_and_releases_account_slot` |
| Selected read cost | Cheap native mailbox change tokens avoid unchanged snapshots; FETCH resolves only requested UIDs through the membership index | `ten_thousand_messages_use_indexed_fetch_and_single_idle_membership_reads` |
| RENAME notifications | Emit exact mailbox identities and descendants with OLDNAME; wildcard characters in names remain literal | `rev2_rename_reports_exact_ids_and_old_names_including_descendants` |

The selected-read cost test uses 10,000 messages and 1,000 single-message FETCHes,
counting native store calls. It also checks 100 IDLE flag events without relisting
the mailbox and one catch-up diff for an unknown UID. Ordinary notifications use
the mailbox's native change counter, including flag changes and removals.

SAVE failures clear `$`, including grammar errors. Successful DELETE of the
selected mailbox returns to authenticated state, with CLOSED in rev2. Missing
subscription rows carry both NonExistent and Noselect; subscription-only children
use CHILDINFO without changing a live mailbox's HasChildren attribute. The empty
LIST pattern remains a delimiter probe when RETURN options are present, and
LIST-STATUS APPENDLIMIT uses the session configuration.

SEARCH scans decoded MIME content without making whole-message lowercased
copies. Its substring matcher uses linear work and memory proportional to the
needle. Content-dependent searches refuse messages above the 64 MiB work limit
with `NO [LIMIT]`; they do not return incomplete matches. A query-weighted
256 MiB scan budget also bounds repeated predicates against one message
(`repeated_content_searches_share_a_per_message_work_budget`). Metadata-only
searches can still match these messages. Unreadable content produces
`NO [UNAVAILABLE]`.

Binary APPEND preserves decoded body octets while storing NUL-bearing leaf
parts with base64 transfer encoding. This keeps ordinary BODY responses valid
IMAP strings; BINARY returns the original decoded octets. Unsupported binary
encodings or NUL in message headers fail with UNKNOWN-CTE. UTF8=ACCEPT also
supports the `UTF8 (~{N}...)` APPEND form.
Embedded-message reparsing shares a 64-level budget across binary APPEND
normalisation (`binary_append_bounds_reparsed_message_nesting`).
BINARY accepts both padded and unpadded MIME base64. RFC822 and RFC822.TEXT set
Seen; RFC822.HEADER and read-only FETCHes do not. RENAME INBOX retains each
membership's flags and user keywords.

Capabilities come from an implementation allowlist. Operator overrides can
select a subset but cannot add unimplemented extensions. APPENDLIMIT always
comes from `imap_max_literal_bytes`. LITERAL+ permits non-synchronising literals
up to the configured cap; a LITERAL- override limits non-synchronising literals
to 4096 octets while permitting larger synchronising literals. Command-wide
allocation and literal-count limits apply as well. Input that cannot be safely
drained closes the connection instead of being interpreted as new commands.
The two protocol capabilities remain present even with an operator override.

## Original refusal inventory

These are the refusal sites found before implementation. Comments and tests
containing the same wording are included so an obsolete phase note cannot be
mistaken for a remaining protocol limit.

| Original site under `services/maild/src/imap/` | Resolution |
|---|---|
| `capabilities.rs`: unimplemented overrides accepted; LITERAL- filtered; stale IDLE examples | Implementation allowlist, dynamic APPENDLIMIT and literal sizing policy |
| `codec.rs`: unstripped literal refusal; mid-command literal refusal; SEARCH-only multi-literal documentation | General segmented session framing, retaining bounded refusal and FramingLost paths |
| `op/fetch.rs`: part-addressing deferrals, BODY refusal and macro refusals | MIME traversal, non-extensible BODY and macro expansion |
| `op/search.rs`: SEARCH RETURN refusal | ESEARCH and session SEARCHRES state |
| `op/status.rs`: SIZE/DELETED deferrals and unsupported-item rejection | Membership aggregates and APPENDLIMIT; unknown items still fail |
| `op/list.rs`: non-empty reference and extension-option refusals | Extended parser, reference resolution and LIST-STATUS |
| `op/lsub.rs`: non-empty reference refusal | Same reference resolution as LIST |
| `op/rename.rs`: INBOX deferral | Atomic storage operation retaining the INBOX container |
| `op/store.rs`: user keywords described as deferred | Documentation corrected; per-membership keywords remain supported |
| `op/create.rs`: transactional parent-walk deferral | Parent creation remains incremental; message operations use atomic transactions. A failed leaf creation may leave newly created parents, which a retry reuses |
| `op/select.rs`: CONDSTORE/QRESYNC parameter refusal; `op/fetch.rs`: MODSEQ refusal; `op/store.rs`: UNCHANGEDSINCE deferral | Intentional dependency on the complete imap-ext implementation; not advertised or enabled here |
| `sasl.rs`: unsupported proxy authzid | Deliberate: empty authzid and authzid equal to authcid work; cross-account proxy authentication is not offered |
| `op/authenticate.rs`: unsupported SASL mechanism | Deliberate rejection of mechanisms outside AUTH=PLAIN and AUTH=LOGIN |
| `op/check.rs`: “deferred mailbox state”; `session.rs`: matching CHECK comment | Descriptive only: CHECK has no pending storage work to flush |
| `session.rs`: unknown UID inner command and generic phase refusal | Invalid commands remain BAD; all standard UID subcommands are dispatched |
| `session.rs`: STARTTLS refusal | Explicit implicit-TLS-only policy; no cleartext IMAP listener is offered |

## Additional RFC 9051 audit findings

The audit also checked server requirements beyond the refusal strings:

| Requirement group | Finding and resolution |
|---|---|
| Sections 4, 5.1 and Appendix A: strings and names | Added UTF-8 negotiation, modified UTF-7 compatibility, NFC validation and literal-kind tracking |
| Sections 2.3.1.1 and 6.3.4: recreated mailbox identity | Persistent UIDVALIDITY high-water mark for new mailboxes, seeded from current wire identities and time. Legacy millisecond values convert to their existing seconds value, preserving client caches. RENAME allocates a fresh value only for a renamed mailbox whose identity collides with another live mailbox or a recorded deleted destination name. `legacy_identities_migrate_before_login_and_cannot_collide_after_rename` and `rename_repairs_only_colliding_live_or_deleted_name_identities` cover both paths |
| Sections 5.2 and 7.5.1: mailbox updates and MSN stability | Added command-time refresh and preserved views while expunge notifications are prohibited |
| Sections 6.3.2–6.3.3: selection data and read-only flags | Accurate LIST attributes and OLDNAME in rev2; CLOSED boundaries on reselection, including failed SELECT; RECENT/UNSEEN in rev1; empty PERMANENTFLAGS for EXAMINE. Covered by `rev2_reselection_marks_the_closed_boundary_including_failure` and `examine_fetch_and_close_unselect_have_distinct_effects` |
| Section 6.3.4: existing mailbox creation | Corrected old idempotent CREATE INBOX and CREATE existing/ behaviour to NO |
| Sections 6.3.5–6.3.8: mailbox lifecycle | DELETE removes memberships atomically, protects children and INBOX; subscription names persist |
| Sections 6.3.12 and 6.4.7: mutation failure | Added atomic MULTIAPPEND and COPY, including UID allocation rollback |
| Sections 6.4.1–6.4.2: CLOSE and UNSELECT | Corrected CLOSE’s missing expunge; UNSELECT remains non-expunging |
| Sections 6.4.4–6.4.5 and 7.5.2: search and body data | Added ESEARCH, SAVE lifetime, charset handling, body forms, encoded sections and decoded binary sizes |
| Sections 6.1.3, 7.1 and 7.2: responses | Final capability list after authentication; operational UNAVAILABLE errors; shutdown BYE |
| Sections 6.4.8–6.4.9: MOVE and UIDPLUS | Retained COPYUID-before-EXPUNGE ordering, UID EXPUNGE filtering and implicit UID FETCH items |

UID ordering, immutable message identities, authentication, NAMESPACE,
UNSELECT, literal abuse caps and the existing MOVE/UIDPLUS behaviours retain
their original regression suites. The release tests are necessary evidence;
this document does not claim certification by every mail client.

CONDSTORE, QRESYNC, MODSEQ, UNCHANGEDSINCE and STATUS HIGHESTMODSEQ are integration
hooks for `work/maild-imap-ext`. ENABLE ignores those names and omits an empty
ENABLED response until that complete implementation lands. STARTTLS remains refused
under the implicit TLS policy.

Protocol references: [RFC 9051](https://www.rfc-editor.org/rfc/rfc9051),
[BINARY](https://www.rfc-editor.org/rfc/rfc3516),
[ENABLE](https://www.rfc-editor.org/rfc/rfc5161),
[SEARCHRES](https://www.rfc-editor.org/rfc/rfc5182),
[LIST-EXTENDED](https://www.rfc-editor.org/rfc/rfc5258),
[UTF8=ACCEPT](https://www.rfc-editor.org/rfc/rfc6855),
[APPENDLIMIT](https://www.rfc-editor.org/rfc/rfc7889).
