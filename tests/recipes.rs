// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
mod support;

use ferriprint::{
    config::ProfileSettings,
    domain::{JobTemplate, Margins, MediaRequest, Resolution, ResolvedJob},
    profile::{PpdInventory, PrinterProfile},
    recipes::{Recipe, validate_key},
    wire::*,
};
use ipp::prelude::*;
use std::{collections::BTreeMap, io::Write, sync::atomic::Ordering};
use support::*;

const PPD: &[u8] = include_bytes!("fixtures/native.ppd");
const PDF: &str = "application/pdf";
const OP: DelimiterTag = DelimiterTag::OperationAttributes;
const JOB: DelimiterTag = DelimiterTag::JobAttributes;
const PRINTER: DelimiterTag = DelimiterTag::PrinterAttributes;

fn recipe_settings() -> ProfileSettings {
    let mut settings: ProfileSettings =
        toml::from_str(include_str!("../examples/profile.toml")).unwrap();
    settings.resolutions = vec![Resolution { x: 300, y: 300 }, Resolution { x: 600, y: 300 }];
    let normal = Recipe {
        display_name: "A4 normal, bordered".into(),
        media: "iso_a4_210x297mm".into(),
        borderless: false,
        source: "main".into(),
        media_type: "stationery".into(),
        quality: 4,
        color_mode: "color".into(),
        sides: "one-sided".into(),
        copies: 2,
        orientation: 4,
        resolution: Resolution { x: 300, y: 300 },
        print_scaling: "fit".into(),
    };
    let photo = Recipe {
        display_name: "A3 randlos – hoch".into(),
        media: "iso_a3_297x420mm".into(),
        borderless: true,
        media_type: "photographic-glossy".into(),
        quality: 5,
        copies: 3,
        orientation: 6,
        print_scaling: "fill".into(),
        ..normal.clone()
    };
    let duplex = Recipe {
        display_name: "Duplex monochrome".into(),
        source: "tray-1".into(),
        quality: 3,
        color_mode: "monochrome".into(),
        sides: "two-sided-long-edge".into(),
        copies: 4,
        orientation: 5,
        resolution: Resolution { x: 600, y: 300 },
        print_scaling: "auto-fit".into(),
        ..normal.clone()
    };
    settings.recipes = BTreeMap::from([
        ("A4normal".into(), normal),
        ("A3borderlessHigh".into(), photo),
        ("duplex".into(), duplex),
    ]);
    settings
}

fn profile(settings: &ProfileSettings) -> PrinterProfile {
    PrinterProfile::from_ppd_bytes(PPD, settings).unwrap()
}

fn assert_resolved_eq(actual: &ResolvedJob, expected: &ResolvedJob) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn complete_recipes_persist_and_never_inherit_changed_defaults() {
    let settings = recipe_settings();
    let before = profile(&settings);
    let normal = before
        .resolve_recipe("A4normal", &JobTemplate::default(), PDF)
        .unwrap();
    assert_eq!(normal.options["PageSize"], "A4");
    assert_eq!(
        normal.media.margins,
        Margins {
            left: 300,
            bottom: 300,
            right: 300,
            top: 300
        }
    );
    let photo = before
        .resolve_recipe("A3borderlessHigh", &JobTemplate::default(), PDF)
        .unwrap();
    assert_eq!(photo.options["PageSize"], "TA3");
    assert_eq!(photo.options["Quality"], "Fine");
    assert_eq!(photo.options["MediaType"], "Photo");
    assert_eq!(photo.media.margins, Margins::default());
    let duplex = before
        .resolve_recipe("duplex", &JobTemplate::default(), PDF)
        .unwrap();
    assert_eq!(duplex.options["Duplex"], "DuplexNoTumble");
    assert_eq!(duplex.options["InputSlot"], "Cassette");
    assert_eq!(duplex.options["ColorModel"], "Gray");
    assert_eq!(duplex.resolution, Resolution { x: 600, y: 300 });
    assert_eq!((normal.copies, photo.copies, duplex.copies), (2, 3, 4));
    assert_eq!(
        (normal.orientation, photo.orientation, duplex.orientation),
        (4, 6, 5)
    );
    assert_eq!(
        (
            &normal.print_scaling[..],
            &photo.print_scaling[..],
            &duplex.print_scaling[..]
        ),
        ("fit", "fill", "auto-fit")
    );

    let mut changed = settings.clone();
    changed.defaults.media = "iso_a3_297x420mm".into();
    changed.defaults.source = "tray-1".into();
    changed.defaults.quality = 5;
    changed.defaults.color_mode = "monochrome".into();
    changed.defaults.sides = "two-sided-short-edge".into();
    changed.resolutions.reverse();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.toml");
    std::fs::write(&path, toml::to_string_pretty(&changed).unwrap()).unwrap();
    let reopened = ProfileSettings::load(&path).unwrap();
    assert_eq!(reopened.recipes, settings.recipes);
    let after = profile(&reopened);
    assert_ne!(
        before.capabilities().defaults.media,
        after.capabilities().defaults.media
    );
    for key in before.recipes().keys() {
        assert_resolved_eq(
            &after
                .resolve_recipe(key, &JobTemplate::default(), PDF)
                .unwrap(),
            &before.recipes()[key].resolved,
        );
    }
}

#[test]
fn explicit_overrides_merge_members_and_replace_size_without_stale_margins() {
    let settings = recipe_settings();
    let profile = profile(&settings);
    let override_all = JobTemplate {
        media_col: Some(MediaRequest {
            source: Some("tray-1".into()),
            media_type: Some("stationery".into()),
            ..Default::default()
        }),
        copies: Some(9),
        quality: Some(5),
        color_mode: Some("monochrome".into()),
        sides: Some("two-sided-short-edge".into()),
        orientation: Some(6),
        resolution: Some(Resolution { x: 600, y: 300 }),
        print_scaling: Some("auto".into()),
        ..Default::default()
    };
    let resolved = profile
        .resolve_recipe("A4normal", &override_all, PDF)
        .unwrap();
    assert_eq!(resolved.options["PageSize"], "A4");
    assert_eq!(resolved.options["InputSlot"], "Cassette");
    assert_eq!(resolved.options["Quality"], "Fine");
    assert_eq!(resolved.options["ColorModel"], "Gray");
    assert_eq!(resolved.options["Duplex"], "DuplexTumble");
    assert_eq!(resolved.copies, 9);
    assert_eq!(resolved.orientation, 6);
    assert_eq!(resolved.resolution, Resolution { x: 600, y: 300 });
    assert_eq!(resolved.print_scaling, "auto");
    assert_eq!(
        resolved.media.margins,
        profile.recipes()["A4normal"].resolved.media.margins
    );

    for request in [
        JobTemplate {
            media: Some("iso_a4_210x297mm".into()),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                name: Some("iso_a4_210x297mm".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                width: Some(21000),
                height: Some(29700),
                ..Default::default()
            }),
            ..Default::default()
        },
    ] {
        let resolved = profile
            .resolve_recipe("A3borderlessHigh", &request, PDF)
            .unwrap();
        assert_eq!(resolved.options["PageSize"], "A4");
        assert_eq!(
            (resolved.media.width, resolved.media.height),
            (21000, 29700)
        );
        assert_eq!(resolved.media.margins.left, 300);
        assert_eq!(resolved.media_type, "photographic-glossy");
        assert_eq!(resolved.quality, 5);
        assert_eq!(resolved.copies, 3);
    }
    let letter = profile
        .resolve_recipe(
            "A3borderlessHigh",
            &JobTemplate {
                media_col: Some(MediaRequest {
                    width: Some(21590),
                    height: Some(27940),
                    ..Default::default()
                }),
                ..Default::default()
            },
            PDF,
        )
        .unwrap();
    assert_eq!(letter.options["PageSize"], "Letter");
    assert!(letter.media.margins.left > 0);

    let bordered = profile
        .resolve_recipe(
            "A3borderlessHigh",
            &JobTemplate {
                media: Some("iso_a3_297x420mm".into()),
                ..Default::default()
            },
            PDF,
        )
        .unwrap();
    assert_eq!(
        bordered.options["PageSize"], "A3",
        "explicit media replaces the recipe's margin selection too"
    );
    let fallback = JobTemplate {
        media: Some("iso_a4_210x297mm".into()),
        media_col: Some(MediaRequest {
            name: Some("iso_a3_297x420mm".into()),
            left: Some(0),
            bottom: Some(0),
            right: Some(0),
            top: Some(0),
            ..Default::default()
        }),
        quality: Some(5),
        ..Default::default()
    };
    assert_eq!(
        profile
            .resolve_recipe("A4normal", &fallback, PDF)
            .unwrap()
            .options["PageSize"],
        "TA3"
    );
    assert_eq!(
        profile.resolve(&fallback, PDF).unwrap().options["PageSize"],
        "TA3",
        "retain the existing media-col-wins IPP policy"
    );
}

#[test]
fn unsupported_overrides_and_internally_conflicting_collections_are_rejected() {
    let settings = recipe_settings();
    let profile = profile(&settings);
    let invalid = [
        JobTemplate {
            sides: Some("two-sided-long-edge".into()),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                source: Some("tray-1".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        JobTemplate {
            quality: Some(3),
            ..Default::default()
        },
        JobTemplate {
            copies: Some(0),
            ..Default::default()
        },
        JobTemplate {
            orientation: Some(7),
            ..Default::default()
        },
        JobTemplate {
            resolution: Some(Resolution { x: 1200, y: 1200 }),
            ..Default::default()
        },
        JobTemplate {
            print_scaling: Some("stretch".into()),
            ..Default::default()
        },
        JobTemplate {
            color_mode: Some("sepia".into()),
            ..Default::default()
        },
        JobTemplate {
            media: Some("iso_a2_420x594mm".into()),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                width: Some(21000),
                ..Default::default()
            }),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                name: Some("iso_a4_210x297mm".into()),
                width: Some(29700),
                height: Some(42000),
                ..Default::default()
            }),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                top: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
        JobTemplate {
            media_col: Some(MediaRequest {
                media_type: Some("unmapped".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    for request in invalid {
        assert!(
            profile
                .resolve_recipe("A3borderlessHigh", &request, PDF)
                .is_err(),
            "accepted {request:?}"
        );
    }
    assert_eq!(
        profile
            .resolve_recipe("a4normal", &JobTemplate::default(), PDF)
            .unwrap_err()
            .attribute,
        "recipe"
    );
    assert_eq!(
        profile
            .resolve_recipe(
                "A4normal",
                &JobTemplate::default(),
                "application/postscript"
            )
            .unwrap_err()
            .attribute,
        "document-format"
    );
}

#[test]
fn every_recipe_field_is_required_and_complete_definitions_obey_the_ppd() {
    let settings = recipe_settings();
    let value = serde_json::to_value(&settings.recipes["A4normal"]).unwrap();
    for key in value.as_object().unwrap().keys() {
        let mut incomplete = value.clone();
        incomplete.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<Recipe>(incomplete).is_err(),
            "inherited missing {key}"
        );
    }
    let mut unknown_field = value;
    unknown_field
        .as_object_mut()
        .unwrap()
        .insert("native_options".into(), serde_json::json!({}));
    assert!(serde_json::from_value::<Recipe>(unknown_field).is_err());
    for key in ["A4normal", "A3borderlessHigh", "duplex", "office_2-a"] {
        validate_key(key).unwrap();
    }
    for key in ["", "é", "space key", "../profile", "-leading", "key\n"] {
        assert!(validate_key(key).is_err());
    }
    assert!(validate_key(&"A".repeat(128)).is_err());
    validate_key(&"A".repeat(127)).unwrap();

    type RecipeMutation = fn(&mut Recipe);
    let invalid: &[(&str, RecipeMutation)] = &[
        ("unsupported borderless variant", |recipe| {
            recipe.borderless = true
        }),
        ("unknown size", |recipe| {
            recipe.media = "iso_a2_420x594mm".into()
        }),
        ("unknown source", |recipe| recipe.source = "unmapped".into()),
        ("unknown material", |recipe| {
            recipe.media_type = "unmapped".into()
        }),
        ("unknown quality", |recipe| recipe.quality = 6),
        ("unknown color", |recipe| recipe.color_mode = "sepia".into()),
        ("unknown sides", |recipe| recipe.sides = "tumble".into()),
        ("invalid copies", |recipe| recipe.copies = 1000),
        ("invalid orientation", |recipe| recipe.orientation = 2),
        ("unsupported input resolution", |recipe| {
            recipe.resolution = Resolution { x: 1, y: 1 }
        }),
        ("unknown scaling", |recipe| {
            recipe.print_scaling = "stretch".into()
        }),
        ("empty display label", |recipe| {
            recipe.display_name = " ".into()
        }),
        ("control in display label", |recipe| {
            recipe.display_name = "line\nbreak".into()
        }),
        ("PPD material/source constraint", |recipe| {
            recipe.media_type = "photographic-glossy".into();
            recipe.source = "tray-1".into();
        }),
        ("PPD borderless/duplex constraint", |recipe| {
            recipe.media = "iso_a3_297x420mm".into();
            recipe.borderless = true;
            recipe.sides = "two-sided-long-edge".into();
        }),
    ];
    for (description, change) in invalid {
        let mut invalid_settings = settings.clone();
        change(invalid_settings.recipes.get_mut("A4normal").unwrap());
        assert!(
            PrinterProfile::from_ppd_bytes(PPD, &invalid_settings).is_err(),
            "accepted {description}"
        );
    }
    let mut invalid_settings = settings.clone();
    invalid_settings
        .recipes
        .insert("invalid key".into(), settings.recipes["A4normal"].clone());
    assert!(PrinterProfile::from_ppd_bytes(PPD, &invalid_settings).is_err());
    let mut oversized = settings;
    for number in 0..129 {
        oversized.recipes.insert(
            format!("entry{number}"),
            oversized.recipes["A4normal"].clone(),
        );
    }
    assert!(PrinterProfile::from_ppd_bytes(PPD, &oversized).is_err());
}

#[test]
fn acquired_ppd_byte_apis_share_bounded_gzip_decoding_and_inventory() {
    let settings = recipe_settings();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(PPD).unwrap();
    let compressed = encoder.finish().unwrap();
    let inventory = PpdInventory::from_bytes(&compressed).unwrap();
    let borderless = inventory
        .pages
        .iter()
        .find(|page| page.native == "TA3")
        .unwrap();
    assert_eq!(borderless.media.margins, Margins::default());
    let compressed_profile = PrinterProfile::from_ppd_bytes(&compressed, &settings).unwrap();
    assert_eq!(
        compressed_profile.recipes()["A3borderlessHigh"]
            .resolved
            .options["PageSize"],
        "TA3"
    );
    assert_eq!(
        compressed_profile.recipes()["A3borderlessHigh"]
            .resolved
            .options["Quality"],
        "Fine"
    );
    assert!(PpdInventory::from_bytes(&[0x1f, 0x8b, 0]).is_err());
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    assert!(PpdInventory::from_bytes(&oversized).is_err());
    assert!(PrinterProfile::from_ppd_bytes(&oversized, &settings).is_err());
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&oversized).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(PpdInventory::from_bytes(&compressed).is_err());
}

fn copied_ticket(front: &Frontend, preset: &IppValue, operation: Operation) -> IppRequestResponse {
    let IppValue::Collection(members) = preset else {
        panic!("preset was not a collection")
    };
    let mut request = front.request(operation, None);
    for (key, value) in members {
        if !matches!(key.as_str(), "preset-name" | "preset-category") {
            add(&mut request, JOB, key.as_str(), value.clone()).unwrap();
        }
    }
    add(&mut request, OP, "document-format", mime(PDF).unwrap()).unwrap();
    request
}

#[tokio::test]
async fn standard_presets_are_filtered_and_copied_by_clients_into_real_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let config = config(directory.path(), &receiver.uri(), 0);
    let settings = recipe_settings();
    save_settings(&config, &settings);
    let front = Frontend::start(config.clone()).await;
    let mut request = front.request(Operation::GetPrinterAttributes, None);
    add(
        &mut request,
        OP,
        "requested-attributes",
        keyword("job-presets-supported").unwrap(),
    )
    .unwrap();
    let response = front.send(request, &[]).await;
    assert_eq!(response.header().status_code(), StatusCode::SuccessfulOk);
    assert_eq!(
        response
            .attributes()
            .first_of(PRINTER)
            .unwrap()
            .attributes()
            .len(),
        1
    );
    let presets = values(attr(&response, PRINTER, "job-presets-supported").unwrap());
    let names: Vec<_> = presets
        .iter()
        .map(|preset| {
            let IppValue::Collection(members) = preset else {
                panic!("preset was not a collection")
            };
            assert_eq!(
                members.get("preset-category").and_then(str_value),
                Some("site")
            );
            assert!(
                !members.contains_key("media"),
                "a full collection needs no conflicting fallback"
            );
            assert!(!members.contains_key("job-preset-name"));
            members.get("preset-name").and_then(str_value).unwrap()
        })
        .collect();
    for required in ["A3borderlessHigh", "A4normal", "duplex"] {
        assert!(names.contains(&required));
    }
    for preset in presets {
        let IppValue::Collection(members) = preset else {
            unreachable!()
        };
        let key = members.get("preset-name").and_then(str_value).unwrap();
        let reply = front
            .send(
                copied_ticket(&front, preset, Operation::PrintJob),
                b"%PDF-1.7\nnonprinting recipe fixture\n%%EOF",
            )
            .await;
        assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
        let id = attr(&reply, JOB, "job-id").and_then(integer).unwrap();
        front.app.engine.tick().await.unwrap();
        let job = front.app.engine.get_job(id).await.unwrap().unwrap();
        let received = receiver.device.jobs.lock().last().unwrap().clone();
        let (
            page,
            quality,
            source,
            material,
            color,
            sides,
            copies,
            orientation,
            resolution,
            scaling,
        ) = match key {
            "A4normal" => (
                "A4",
                "Standard",
                "Rear",
                "Plain",
                "RGB",
                "None",
                2,
                4,
                (300, 300),
                "fit",
            ),
            "A3borderlessHigh" => (
                "TA3",
                "Fine",
                "Rear",
                "Photo",
                "RGB",
                "None",
                3,
                6,
                (300, 300),
                "fill",
            ),
            "duplex" => (
                "A4",
                "Economy",
                "Cassette",
                "Plain",
                "Gray",
                "DuplexNoTumble",
                4,
                5,
                (600, 300),
                "auto-fit",
            ),
            other => panic!("unexpected test recipe {other}"),
        };
        for (option, expected) in [
            ("PageSize", page),
            ("Quality", quality),
            ("InputSlot", source),
            ("MediaType", material),
            ("ColorModel", color),
            ("Duplex", sides),
        ] {
            assert_eq!(
                received.options.get(option).map(String::as_str),
                Some(expected),
                "{key}: {option}"
            );
        }
        assert_eq!(received.copies, copies);
        assert_eq!(received.orientation, orientation);
        assert_eq!(received.resolution, Some(resolution));
        assert_eq!(received.print_scaling.as_deref(), Some(scaling));
        assert_eq!(
            job.resolved.media.margins,
            if key == "A3borderlessHigh" {
                Margins::default()
            } else {
                Margins {
                    left: 300,
                    bottom: 300,
                    right: 300,
                    top: 300,
                }
            }
        );
        assert_eq!(received.format, PDF);
        receiver.complete(received.id);
        front.app.engine.tick().await.unwrap();

        let mut changed = settings.clone();
        changed.recipes.get_mut(key).unwrap().copies = 8;
        save_settings(&config, &changed);
        let reopened = ProfileSettings::load(&config.profile_path("demo")).unwrap();
        assert_eq!(profile(&reopened).recipes()[key].resolved.copies, 8);
        let historical = front.app.engine.get_job(id).await.unwrap().unwrap();
        assert_resolved_eq(&historical.resolved, &job.resolved);
    }
    let mut quality_only = front.request(Operation::GetPrinterAttributes, None);
    add(
        &mut quality_only,
        OP,
        "requested-attributes",
        keyword("print-quality-supported").unwrap(),
    )
    .unwrap();
    let quality_only = front.send(quality_only, &[]).await;
    assert!(attr(&quality_only, PRINTER, "print-quality-supported").is_some());
    assert!(attr(&quality_only, PRINTER, "job-presets-supported").is_none());
    let all = front
        .send(front.request(Operation::GetPrinterAttributes, None), &[])
        .await;
    assert!(attr(&all, PRINTER, "job-presets-supported").is_some());
    assert!(
        !string_list(attr(&all, PRINTER, "job-creation-attributes-supported"))
            .contains(&"job-preset-name".into())
    );

    let attempts = receiver.device.attempts.load(Ordering::SeqCst);
    let borderless=presets.iter().find(|preset|matches!(preset,IppValue::Collection(members) if members.get("preset-name").and_then(str_value)==Some("A3borderlessHigh"))).unwrap();
    let mut invalid = copied_ticket(&front, borderless, Operation::PrintJob);
    invalid
        .attributes_mut()
        .groups_mut()
        .iter_mut()
        .find(|group| group.tag() == JOB)
        .unwrap()
        .attributes_mut()
        .retain(|attribute| attribute.name().as_str() != "sides");
    add(
        &mut invalid,
        JOB,
        "sides",
        keyword("two-sided-long-edge").unwrap(),
    )
    .unwrap();
    let invalid = front.send(invalid, b"%PDF-1.7\n%%EOF").await;
    assert_eq!(
        invalid.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
    let mut selector = front.request(Operation::PrintJob, None);
    add(
        &mut selector,
        JOB,
        "job-preset-name",
        name("A4normal").unwrap(),
    )
    .unwrap();
    let selector = front.send(selector, b"%PDF-1.7\n%%EOF").await;
    assert_eq!(
        selector.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
    front.app.engine.tick().await.unwrap();
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), attempts);
    let historical = front.app.engine.list_jobs("demo").await.unwrap();
    drop(front);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let reopened = ferriprint::server::App::open(config).await.unwrap();
    for old in historical {
        let persisted = reopened.engine.get_job(old.id).await.unwrap().unwrap();
        assert_eq!(persisted.uuid, old.uuid);
        assert_resolved_eq(&persisted.resolved, &old.resolved);
    }
}
