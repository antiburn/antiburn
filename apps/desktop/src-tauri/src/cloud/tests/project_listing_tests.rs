use super::*;
use crate::cloud::DestinationBinding;

fn open(progress: Option<ListingProgress>, key: [u8; 32]) -> ListingLookups {
    ListingLookups::new(progress, key, Instant::now() + PROGRESS_TTL)
}

fn plain(_: &mut CheckoutOriginCache, path: &str, _: Instant) -> Context {
    Context::plain(path)
}

#[test]
fn continuation_reuses_finished_paths_and_reaches_a_later_path() {
    let key = [0; 32];
    let mut first = open(None, key);
    assert!(
        first
            .inspect("/finished", |_, path, deadline| {
                assert!(deadline <= Instant::now() + PATH_LOOKUP_LIMIT);
                Context::plain(path)
            })
            .complete
    );
    first.start_before = Instant::now();
    assert!(
        first
            .inspect("/later", |_, _, _| panic!("path must be deferred"))
            .inspection_incomplete
    );
    let (progress, retry) = first.finish();
    assert!(retry);
    let mut next = open(progress, key);
    assert!(
        next.inspect("/finished", |_, _, _| panic!("repeat check"))
            .complete
    );
    assert!(next.inspect("/later", plain).complete);
    assert!(matches!(next.finish(), (None, false)));
}

#[test]
fn continuation_requires_new_progress_and_resumable_work() {
    let key = [0; 32];
    let mut first = open(None, key);
    first.inspect("/unreadable", |_, path, _| Context::unavailable(path, true));
    assert!(
        first
            .inspect(&format!("/{}", "x".repeat(4096)), |_, _, _| {
                panic!("oversized paths must be terminal")
            })
            .inspection_incomplete
    );
    assert!(!first.deferred);
    first.start_before = Instant::now();
    first.inspect("/later", |_, _, _| panic!("path must be deferred"));
    let (progress, retry) = first.finish();
    assert!(retry);
    let mut next = open(progress, key);
    next.start_before = Instant::now();
    assert!(
        next.inspect("/unreadable", |_, _, _| panic!("repeat check"))
            .inspection_incomplete
    );
    assert!(!next.deferred);
    next.inspect("/later", |_, _, _| panic!("path must be deferred"));
    assert!(matches!(next.finish(), (Some(_), false)));
}

#[test]
fn a_context_that_exceeds_available_space_keeps_a_terminal_result() {
    let mut lookups = open(None, [0; 32]);
    lookups.progress.bytes = CACHE_BYTES - 160;
    let oversized = lookups.inspect("/one", |_, path, _| {
        let mut result = Context::plain(path);
        result.folders.push("x".repeat(4096));
        result
    });
    assert!(oversized.inspection_incomplete);
    lookups.start_before = Instant::now();
    assert!(
        lookups
            .inspect("/one", |_, _, _| panic!("repeat oversized result"))
            .inspection_incomplete
    );
    assert!(
        lookups
            .inspect("/two", |_, _, _| panic!("cannot retain a result"))
            .inspection_incomplete
    );
    assert!(matches!(lookups.finish(), (None, false)));
}

#[test]
fn listing_scope_fences_progress_and_does_not_extend_expiration() {
    let mut authority = ListingAuthority {
        binding: DestinationBinding {
            account_id: "account".into(),
            destination_id: "personal".into(),
        },
        generation: 1,
        expires_at: 100,
    };
    let key = ListingProgress::key(&authority, 2, None, &[], &[]);
    assert_ne!(key, ListingProgress::key(&authority, 3, None, &[], &[]));
    assert_ne!(
        key,
        ListingProgress::key(
            &authority,
            2,
            Some(&Cursor {
                environment: "native".into(),
                cwd: "/folder".into()
            }),
            &[],
            &[],
        )
    );
    assert_ne!(
        key,
        ListingProgress::key(
            &authority,
            2,
            None,
            &[Folder {
                path: "/source".into(),
                selected: false,
                ..Folder::default()
            }],
            &[]
        )
    );
    authority.generation += 1;
    let changed_key = ListingProgress::key(&authority, 2, None, &[], &[]);
    assert_ne!(key, changed_key);

    let mut first = open(None, key);
    first.inspect("/folder", plain);
    let changed = open(Some(first.progress), changed_key);
    assert!(changed.progress.contexts.is_empty());

    let mut first = open(None, key);
    first.inspect("/folder", plain);
    let expires_at = first.progress.expires_at;
    let mut resumed = open(Some(first.progress), key);
    assert_eq!(resumed.progress.expires_at, expires_at);
    assert!(!resumed.progress.contexts.is_empty());
    resumed.progress.expires_at = Instant::now();
    assert!(
        open(Some(resumed.progress), key)
            .progress
            .contexts
            .is_empty()
    );
}
