# Code-scanning review

Reviewed on 2026-11-16 against CodeQL 2.27.4, Rust analysis 2182591195 of
63bf07f. The SARIF code flows, concrete Rust types and FFI ownership contracts
were reviewed before classifying the alerts below. No query, language or vendor
directory is excluded from scanning.

## Browser protocol dispatcher: alert 10

The browser verification runner received response IDs from its loopback debugging
connection and immediately invoked the corresponding map entry. The dispatcher
now rejects malformed messages, requires a positive safe-integer ID and checks
that the locally registered callback is a function before calling it. The entry
is removed before invocation, so duplicate responses cannot resolve it again.
Unknown IDs never dispatch to object properties or other targets.

`node --test scripts/cdp-messages.test.mjs` covers registered/duplicate responses,
malformed input, prototype-like names, unknown IDs, non-function entries and
exception notifications. The browser suite exercises the actual protocol client.

## Vendored GLib: alerts 3–9

These are false positives in the reported flows, not source fixes or accepted
security risks. Dismissal comments link back to this review. The existing GLib
soundness backport and its Linux regression remain in place.

| Alert | Reported path | Evidence |
| --- | --- | --- |
| 3 | `auto/functions.rs:857` → generic `from_glib` → `log.rs:85` | `uuid_string_is_valid` returns a Rust `bool` from a C `gboolean`. That instantiation selects the boolean conversion, not `LogLevel::from_glib(u32)`. The traced value is a validation result, not the UUID string or a credential. There is no logging operation on this path. |
| 4 | Charset/error out-pointers → `from_glib_none` → `object.rs:470` | Charset calls use `*const c_char` and return `GString` or `Option<&GStr>`. File-write failures use `*mut GError`, guarded by `error.is_null()`, and return `Error`. These instantiations cannot select the `*mut GObject` implementation dereferenced at this sink. |
| 5 | Error/hostname out-pointers → `from_glib_full` → `object.rs:493` | The traced file-write and URI calls convert `GError`, a path, or an optional hostname. Null errors are checked; optional hostnames support null. None of these concrete conversions is `ObjectRef::from_glib_full(*mut GObject)`. |
| 6 | The same `from_glib_none` sources → boxed test macro | The macro expansion belongs to the vendor's `MyBoxed` test type. String and error conversions cannot dispatch to its boxed-pointer implementation. The generic conversion edges cross incompatible instantiations. |
| 7 | The same `from_glib_none` sources → `value.rs:462` | `copy_into_value` requires an actual `*const GValue`. Charset/error conversion instantiations do not produce or copy `GValue`. The traced source is not a possible argument to this sink. |
| 8 | The same `from_glib_full` sources → `value.rs:470` | `clear_value` requires an actual `*mut GValue`. A `GError`, path, or optional hostname conversion cannot select the value-wrapper clear callback. |
| 9 | `drop_in_place::<T>(p as *mut T)` → `ptr::write(p, null)` in `PtrSlice::truncate` | `p` is a slot in the container's GLib-allocated pointer array. `T: TransparentPtrType` is a pointer-sized owning wrapper; dropping it releases its pointee/reference, not the array slot. The following write clears the still-live slot. The array itself is freed separately by `PtrSlice::drop`, after its elements. This is not use-after-free of the array allocation. |

The complete reported pointer-source sets were reviewed: console charset,
locale charset, file-set-contents, file-set-contents-full and filename-from-URI.
The analysis merges incompatible concrete implementations of the generic
translation helpers. A null-initialized C out-parameter also does not imply it
remains null after the C function fills it.

Relevant contracts are visible in the vendored `auto/functions.rs`,
`manual_functions.rs`, `translate.rs`, `object.rs`, `value.rs` and
`collections/ptr_slice.rs`. GLib documents the
[UUID validator's boolean result](https://docs.gtk.org/glib/func.uuid_string_is_valid.html)
and [the charset output argument](https://docs.gtk.org/glib/func.get_charset.html).

The Linux-only `glib_code_scanning_type_and_container_contracts` regression
exercises UUID validation, charset/error conversions, actual GObject reference
ownership, object/boxed value copying, and truncating/reusing a pointer array.
These checks cover the reviewed contracts; they are not a general memory-safety
proof. Re-review a reintroduced alert if its concrete flow or dependency version
differs from the paths above.

## macOS fixture portability

The earlier macOS failure was a `WouldBlock` read in the mock AI server, not a
compiler error. Accepted sockets are explicitly placed in blocking mode before
the existing read timeout is applied. The fragmented-request regression sends
headers and JSON in separate delayed writes so the reader must wait for more
input. The listener remains nonblocking to retain its bounded accept loop.
