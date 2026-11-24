//! Controller selection and intent queue: identity evidence, visible refusals, and fixed bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::interaction::*;
use oa_display::list::{list_new, push_rect, DisplayError, ListCaps};
use oa_display::palette::BACKGROUND;
use oa_display::project::projection_new;

fn identity(address: u32, seed: u8) -> FlightIdentity {
    FlightIdentity {
        gufi: [seed; 16],
        aircraft_address: address,
        centre: [seed.wrapping_add(1); 4],
        plan_id: [seed.wrapping_add(2); 12],
    }
}

fn view(key: FlightIdentity, generation: u32) -> TargetView {
    TargetView {
        address: key.aircraft_address,
        generation,
        identity_claims: 1,
        identity_present: 1,
        reserved: 0,
        identity: key,
    }
}

// REQ: OA-UI-002, OA-UI-004, OA-UI-005, OA-UI-007, OA-UI-013
#[test]
fn separate_flights_bind_only_by_full_identity() {
    let first = identity(0x123456, 1);
    let second = identity(0x654321, 2);
    let views = [view(first, 4), view(second, 9)];
    let mut state = interaction_new();

    assert_eq!(
        select_target(
            &mut state,
            &views,
            100,
            second.aircraft_address,
            9,
            Some(second)
        ),
        InteractionResult::Applied
    );
    assert_eq!(selected_address(&state), Some(second.aircraft_address));
    assert_eq!(selected_identity(&state), Some(second));
    assert_eq!(
        enqueue_intent(
            &mut state,
            &views,
            101,
            1,
            5_000_000,
            Intent::SetAssignedAltitude(12_000)
        ),
        InteractionResult::Applied
    );
    let command = next_intent(&state).unwrap();
    assert_eq!(command.identity, second);
    assert_eq!(command.source_view_sequence, 101);
    assert_eq!(command.target_generation, 9);
    assert_eq!(command.source_time_micros, 5_000_000);
    assert_eq!(command.intent, Intent::SetAssignedAltitude(12_000));
}

// REQ: OA-UI-007, OA-UI-008, OA-UI-018
#[test]
fn duplicate_address_and_identity_claims_are_visible_refusals() {
    let key = identity(0xAABBCC, 7);
    let duplicate_rows = [view(key, 1), view(key, 1)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(
            &mut state,
            &duplicate_rows,
            1,
            key.aircraft_address,
            1,
            Some(key)
        ),
        InteractionResult::Refused(Refusal::DuplicateAddress)
    );
    assert_eq!(last_refusal(&state), Some(Refusal::DuplicateAddress));

    let mut claimed_twice = view(key, 1);
    claimed_twice.identity_claims = 2;
    assert_eq!(
        select_target(
            &mut state,
            &[claimed_twice],
            2,
            key.aircraft_address,
            1,
            Some(key)
        ),
        InteractionResult::Refused(Refusal::IdentityAmbiguous)
    );
}

// REQ: OA-UI-004, OA-UI-006, OA-UI-009
#[test]
fn raw_track_selection_never_queues_control_and_clear_is_explicit() {
    let key = identity(0x123456, 4);
    let views = [view(key, 2)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &views, 1, key.aircraft_address, 2, None),
        InteractionResult::Applied
    );
    assert_eq!(selection_kind(&state), SelectionKind::RawTrack);
    assert_eq!(
        enqueue_intent(&mut state, &views, 2, 1, 2, Intent::TakeHandoff(77)),
        InteractionResult::Refused(Refusal::RawTrackOnly)
    );
    clear_selection(&mut state);
    assert_eq!(selection_kind(&state), SelectionKind::None);
}

// REQ: OA-UI-008, OA-UI-014, OA-UI-015
#[test]
fn stale_generation_and_backwards_input_sequence_are_refused() {
    let key = identity(0x101010, 3);
    let original = [view(key, 5)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(
            &mut state,
            &original,
            10,
            key.aircraft_address,
            5,
            Some(key)
        ),
        InteractionResult::Applied
    );
    let changed_generation = [view(key, 6)];
    assert_eq!(
        enqueue_intent(
            &mut state,
            &changed_generation,
            11,
            1,
            1,
            Intent::SetInterimAltitude(8_000)
        ),
        InteractionResult::Refused(Refusal::StaleGeneration)
    );
    assert_eq!(
        enqueue_intent(
            &mut state,
            &original,
            11,
            5,
            2,
            Intent::SetInterimAltitude(8_000)
        ),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(
            &mut state,
            &original,
            12,
            5,
            3,
            Intent::ClearInterimAltitude
        ),
        InteractionResult::Refused(Refusal::BackwardsSequence)
    );
}

// REQ: OA-UI-015, OA-UI-017
#[test]
fn address_reuse_cannot_retarget_a_queued_intent() {
    let original_key = identity(0xABCDEF, 1);
    let reused_key = identity(0xABCDEF, 9);
    let original = [view(original_key, 3)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(
            &mut state,
            &original,
            20,
            original_key.aircraft_address,
            3,
            Some(original_key)
        ),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &original, 21, 1, 1, Intent::OfferHandoff(42)),
        InteractionResult::Applied
    );

    assert_eq!(next_intent(&state).unwrap().identity, original_key);
    assert_eq!(
        preflight_oldest(&mut state, &[view(reused_key, 3)], 22),
        InteractionResult::Refused(Refusal::IdentityMismatch)
    );
    assert_eq!(next_intent(&state).unwrap().identity, original_key);
}

// REQ: OA-UI-012, OA-UI-014
#[test]
fn fixed_queue_overflow_is_counted_and_repeated_sequences_do_not_duplicate() {
    let key = identity(0x334455, 6);
    let views = [view(key, 8)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &views, 1, key.aircraft_address, 8, Some(key)),
        InteractionResult::Applied
    );
    for seq in 1..=COMMAND_CAPACITY as u32 {
        assert_eq!(
            enqueue_intent(
                &mut state,
                &views,
                seq + 1,
                seq,
                u64::from(seq),
                Intent::OfferHandoff(seq)
            ),
            InteractionResult::Applied
        );
    }
    assert_eq!(pending_count(&state), COMMAND_CAPACITY);
    assert_eq!(
        enqueue_intent(&mut state, &views, 100, 100, 100, Intent::CancelHandoff(1)),
        InteractionResult::Refused(Refusal::QueueFull)
    );
    assert_eq!(
        enqueue_intent(
            &mut state,
            &views,
            100,
            COMMAND_CAPACITY as u32,
            100,
            Intent::CancelHandoff(1)
        ),
        InteractionResult::Refused(Refusal::BackwardsSequence)
    );
    assert_eq!(pending_count(&state), COMMAND_CAPACITY);
}

// REQ: OA-UI-003
#[test]
fn view_work_cap_is_checked_before_inspection() {
    let key = identity(0x223344, 8);
    let rows = vec![view(key, 1); MAX_TARGET_VIEWS + 1];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &rows, 1, key.aircraft_address, 1, Some(key)),
        InteractionResult::Refused(Refusal::ViewTooLarge)
    );
}

// REQ: OA-UI-016
#[test]
fn completion_must_match_the_head_sequence() {
    let key = identity(0x445566, 5);
    let views = [view(key, 2)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &views, 1, key.aircraft_address, 2, Some(key)),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 2, 7, 900, Intent::AcceptHandoff(31)),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 1, 8, 901, Intent::CancelHandoff(31)),
        InteractionResult::Refused(Refusal::StaleView)
    );
    assert_eq!(
        complete_oldest(&mut state, 8, CommandOutcome::Accepted),
        InteractionResult::Refused(Refusal::CommandSequenceMismatch)
    );
    assert_eq!(next_intent(&state).unwrap().input_sequence, 7);
    assert_eq!(
        complete_oldest(&mut state, 7, CommandOutcome::Accepted),
        InteractionResult::Applied
    );
    assert_eq!(last_completed_sequence(&state), 7);
    assert_eq!(next_intent(&state), None);
}

#[test]
fn refused_sequence_can_be_retried_but_accepted_issue_time_cannot_go_backwards() {
    let key = identity(0x445567, 5);
    let views = [view(key, 2)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &views, 1, key.aircraft_address, 2, Some(key)),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 0, 1, 100, Intent::AcceptHandoff(31)),
        InteractionResult::Refused(Refusal::StaleView)
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 2, 1, 100, Intent::AcceptHandoff(31)),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 3, 2, 99, Intent::CancelHandoff(31)),
        InteractionResult::Refused(Refusal::BackwardsSourceTime)
    );
    assert_eq!(
        enqueue_intent(&mut state, &views, 3, 2, 101, Intent::CancelHandoff(31)),
        InteractionResult::Applied
    );
    assert_eq!(pending_count(&state), 2);
}

#[test]
fn completing_one_flight_does_not_retarget_the_next_queued_flight() {
    let first = identity(0x112233, 1);
    let second = identity(0x445566, 2);
    let first_view = [view(first, 4)];
    let second_view = [view(second, 9)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(
            &mut state,
            &first_view,
            1,
            first.aircraft_address,
            4,
            Some(first)
        ),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &first_view, 2, 1, 1, Intent::OfferHandoff(20)),
        InteractionResult::Applied
    );
    assert_eq!(
        select_target(
            &mut state,
            &second_view,
            3,
            second.aircraft_address,
            9,
            Some(second)
        ),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(&mut state, &second_view, 4, 2, 2, Intent::TakeHandoff(21)),
        InteractionResult::Applied
    );

    assert_eq!(
        complete_oldest(&mut state, 1, CommandOutcome::Accepted),
        InteractionResult::Applied
    );
    let next = next_intent(&state).unwrap();
    assert_eq!(next.identity, second);
    assert_eq!(next.input_sequence, 2);
    assert_eq!(
        preflight_oldest(&mut state, &second_view, 5),
        InteractionResult::Applied
    );
}

// REQ: OA-UI-001, OA-UI-010, OA-UI-011, OA-UI-013
#[test]
fn all_typed_intents_round_trip_and_replay_produces_identical_state() {
    let key = identity(0xabc123, 1);
    let views = [view(key, 4)];
    let intents = [
        Intent::SetAssignedAltitude(12000),
        Intent::ClearAssignedAltitude,
        Intent::SetInterimAltitude(10000),
        Intent::ClearInterimAltitude,
        Intent::OfferHandoff(42),
        Intent::AcceptHandoff(43),
        Intent::TakeHandoff(44),
        Intent::CancelHandoff(45),
    ];
    let mut first = interaction_new();
    let mut second = interaction_new();
    for state in [&mut first, &mut second] {
        assert_eq!(
            select_target(state, &views, 7, key.aircraft_address, 4, Some(key)),
            InteractionResult::Applied
        );
        for (index, intent) in intents.iter().enumerate() {
            let sequence = u32::try_from(index + 1).unwrap();
            assert_eq!(
                enqueue_intent(state, &views, 7, sequence, u64::from(sequence), *intent),
                InteractionResult::Applied
            );
            let captured = next_intent(state).unwrap();
            assert_eq!(captured.identity, key);
            assert_eq!(captured.intent, *intent);
            assert_eq!(captured.source_view_sequence, 7);
            assert_eq!(captured.target_generation, 4);
            assert_eq!(captured.source_time_micros, u64::from(sequence));
            assert_eq!(
                complete_oldest(state, sequence, CommandOutcome::Accepted),
                InteractionResult::Applied
            );
        }
    }
    assert_eq!(first, second);
}

fn display_caps(texts: usize, text_bytes: usize) -> ListCaps {
    ListCaps {
        lines: 0,
        rects: 4,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts,
        text_bytes,
    }
}

fn projection(width: u32, height: u32) -> oa_display::project::Projection {
    projection_new(420_000_000, -880_000_000, 100.0, width, height, 104).unwrap()
}

fn selected_interaction() -> InteractionState {
    let key = identity(0x123456, 3);
    let views = [view(key, 4)];
    let mut state = interaction_new();
    assert_eq!(
        select_target(&mut state, &views, 1, key.aircraft_address, 4, Some(key)),
        InteractionResult::Applied
    );
    assert_eq!(
        enqueue_intent(
            &mut state,
            &views,
            2,
            1,
            10,
            Intent::SetAssignedAltitude(12_000)
        ),
        InteractionResult::Applied
    );
    state
}

// REQ: OA-UI-018, OA-UI-019
#[test]
fn interaction_text_uses_pixel_coordinates_and_follows_the_legend() {
    let state = selected_interaction();
    let proj = projection(640, 400);
    let mut list = list_new(display_caps(2, 128)).unwrap();
    present_interaction(&state, &mut list, &proj).unwrap();
    assert!(list.texts.x[0] > 320 * 16);
    assert_eq!(list.texts.x[0], list.texts.x[1]);
    assert_eq!(list.texts.y, [(104 + 8) * 16, (104 + 8 + 14) * 16]);
    assert_eq!(list.texts.layer, [LAYER_INTERACTION, LAYER_INTERACTION]);
}

// REQ: OA-UI-019, OA-UI-020
#[test]
fn interaction_avoids_the_left_safety_panel_and_uses_bottom_fallbacks() {
    let state = selected_interaction();
    let proj = projection(640, 400);
    let mut left_panel = list_new(display_caps(2, 128)).unwrap();
    push_rect(
        &mut left_panel,
        (4 * 16, 108 * 16),
        (280 * 16, 220 * 16),
        BACKGROUND,
        14,
    )
    .unwrap();
    present_interaction(&state, &mut left_panel, &proj).unwrap();
    assert!(left_panel.texts.x[0] > 320 * 16);
    assert_eq!(left_panel.texts.y[0], 112 * 16);

    let mut full_width_panel = list_new(display_caps(2, 128)).unwrap();
    push_rect(
        &mut full_width_panel,
        (0, 108 * 16),
        (640 * 16, 220 * 16),
        BACKGROUND,
        14,
    )
    .unwrap();
    present_interaction(&state, &mut full_width_panel, &proj).unwrap();
    assert!(full_width_panel.texts.x[0] > 320 * 16);
    assert_eq!(full_width_panel.texts.y[0], (400 - 42 - 26) * 16);
}

// REQ: OA-UI-019, OA-UI-020
#[test]
fn interaction_refuses_when_higher_priority_panel_covers_all_positions() {
    let state = selected_interaction();
    let proj = projection(640, 400);
    let mut list = list_new(display_caps(2, 128)).unwrap();
    push_rect(
        &mut list,
        (0, 104 * 16),
        (640 * 16, 400 * 16),
        BACKGROUND,
        14,
    )
    .unwrap();
    let before = list.clone();
    assert!(matches!(
        present_interaction(&state, &mut list, &proj),
        Err(DisplayError::BadView(
            "no unobscured viewport position fits interaction status"
        ))
    ));
    assert_eq!(list, before);
}

// REQ: OA-UI-019
#[test]
fn narrow_and_short_viewports_refuse_without_mutating_the_list() {
    let state = selected_interaction();
    let mut list = list_new(display_caps(2, 128)).unwrap();
    let before = list.clone();
    assert!(matches!(
        present_interaction(&state, &mut list, &projection(120, 300)),
        Err(DisplayError::BadView(
            "frame is too narrow for interaction status"
        ))
    ));
    assert_eq!(list, before);
    assert!(matches!(
        present_interaction(&state, &mut list, &projection(640, 130)),
        Err(DisplayError::BadView(
            "no unobscured viewport position fits interaction status"
        ))
    ));
    assert_eq!(list, before);
}

// REQ: OA-UI-012, OA-UI-019
#[test]
fn insufficient_or_damaged_text_capacity_refuses_before_list_mutation() {
    let state = selected_interaction();
    let proj = projection(640, 400);
    let mut too_small = list_new(display_caps(1, 128)).unwrap();
    let before = too_small.clone();
    assert!(matches!(
        present_interaction(&state, &mut too_small, &proj),
        Err(DisplayError::Full {
            table: "texts",
            capacity: 1
        })
    ));
    assert_eq!(too_small, before);

    let mut damaged = list_new(display_caps(2, 128)).unwrap();
    damaged.texts.colour.push(1);
    let before = damaged.clone();
    assert!(matches!(
        present_interaction(&state, &mut damaged, &proj),
        Err(DisplayError::BadView(
            "interaction display list text storage is inconsistent"
        ))
    ));
    assert_eq!(damaged, before);
}
