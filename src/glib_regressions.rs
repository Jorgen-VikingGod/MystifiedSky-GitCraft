//! Guards the vendored glib that the desktop shell's GTK bindings pull in.

#[cfg(target_os = "linux")]
#[test]
fn glib_code_scanning_type_and_container_contracts() {
    use glib::{prelude::*, translate::*};
    #[derive(Clone, Debug, PartialEq, glib::Boxed)]
    #[boxed_type(name = "GitCraftScanningFixture")]
    struct BoxedFixture(String);

    let valid: bool = glib::uuid_string_is_valid("f81d4fae-7dec-11d0-a765-00a0c91e6bf6");
    assert!(valid);
    assert!(!glib::uuid_string_is_valid("invalid"));
    let (_, charset) = glib::charset();
    assert!(charset.is_some());
    let folder = tempfile::tempdir().unwrap();
    assert!(glib::file_set_contents(folder.path().join("missing/file"), b"fixture").is_err());
    assert!(glib::filename_from_uri("not-a-file-uri").is_err());

    let object = glib::Object::new::<glib::Object>();
    let raw: *mut glib::gobject_ffi::GObject = object.to_glib_full();
    let owned: glib::Object = unsafe { from_glib_full(raw) };
    let borrowed_raw: *mut glib::gobject_ffi::GObject = object.to_glib_none().0;
    let borrowed: glib::Object = unsafe { from_glib_none(borrowed_raw) };
    assert_eq!(object, owned); assert_eq!(object, borrowed);
    let object_value = object.to_value();
    assert_eq!(object_value.clone().get::<glib::Object>().unwrap(), object);
    let boxed = BoxedFixture("fixture".into());
    assert_eq!(boxed.to_value().clone().get::<BoxedFixture>().unwrap(), boxed);

    let error = |message: &str| glib::Error::new(glib::FileError::Failed, message);
    let mut slice = glib::collections::PtrSlice::from([error("kept"), error("dropped")]);
    slice.truncate(1); assert_eq!(slice[0].message(), "kept");
    slice.push(error("reused")); assert_eq!(slice[1].message(), "reused");
    slice.truncate(0); slice.push(error("after empty"));
    assert_eq!(slice.len(), 1); assert_eq!(slice[0].message(), "after empty");
}

#[cfg(target_os = "linux")]
#[test]
fn glib_variant_string_iterator_uses_a_mutable_ffi_out_pointer() {
    use glib::variant::ToVariant;
    let values = ["alpha", "café", "", "omega"];
    let variant = glib::Variant::array_from_iter::<String>(values.map(|value| value.to_variant()));
    for _ in 0..100 {
        assert_eq!(variant.array_iter_str().unwrap().collect::<Vec<_>>(), values);
        assert_eq!(variant.array_iter_str().unwrap().next(), Some("alpha"));
        assert_eq!(variant.array_iter_str().unwrap().next_back(), Some("omega"));
        assert_eq!(variant.array_iter_str().unwrap().nth(1), Some("café"));
        assert_eq!(variant.array_iter_str().unwrap().nth_back(1), Some(""));
        assert_eq!(variant.array_iter_str().unwrap().last(), Some("omega"));
        let mut mixed = variant.array_iter_str().unwrap();
        assert_eq!(mixed.next(), Some("alpha"));
        assert_eq!(mixed.next_back(), Some("omega"));
        assert_eq!(mixed.next_back(), Some(""));
        assert_eq!(mixed.next(), Some("café"));
        assert_eq!(mixed.next(), None);
    }
}
