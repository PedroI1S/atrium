#[cfg(feature = "ts")]
fn main() {
    use specta::Type;
    use specta_util::Remapper;
    use std::fs;
    use std::path::{Path, PathBuf};

    use atrium_protocol::agent::*;
    use atrium_protocol::policy::*;
    use atrium_protocol::thread::*;
    use atrium_protocol::wire::*;

    let types = specta::Types::default()
        // agent
        .register::<Provider>()
        .register::<AgentEventEnvelope>()
        .register::<AgentEvent>()
        .register::<MessageBody>()
        .register::<MessageKind>()
        .register::<ToolKind>()
        .register::<ToolStatus>()
        .register::<TurnOutcome>()
        .register::<RequestOutcome>()
        .register::<Question>()
        .register::<QuestionOption>()
        .register::<QuestionAnswer>()
        .register::<PlanEntry>()
        .register::<PlanStatus>()
        .register::<Usage>()
        .register::<QuotaWindow>()
        .register::<SessionCommand>()
        .register::<ApprovalDecision>()
        // policy
        .register::<EnforcementLevel>()
        .register::<ToolClass>()
        .register::<Coverage>()
        .register::<ClassCoverage>()
        .register::<ConfigSourceKind>()
        .register::<ConfigSource>()
        .register::<ReasonCode>()
        .register::<EnforcementReason>()
        .register::<EnforcementDescriptor>()
        .register::<Preset>()
        .register::<RuleDecision>()
        .register::<RuleScope>()
        .register::<RememberScope>()
        .register::<RuleMatcher>()
        .register::<RuleId>()
        .register::<ToolSubject>()
        .register::<RuleDraft>()
        .register::<RuleOrigin>()
        .register::<Rule>()
        .register::<PolicyContext>()
        // thread
        .register::<ThreadId>()
        .register::<Cleanup>()
        .register::<ThreadState>()
        .register::<Location>()
        .register::<ThreadSummary>()
        .register::<InboxReason>()
        .register::<InboxItem>()
        // wire
        .register::<EventId>()
        .register::<StoredEvent>()
        .register::<ClientRole>()
        .register::<Authority>()
        .register::<ClientMessage>()
        .register::<DaemonCommand>()
        .register::<ProviderProbe>()
        .register::<ExportFormat>()
        .register::<AgentLaunch>()
        .register::<ServerMessage>()
        .register::<CommandResult>()
        .register::<ErrorCode>();

    // Export u64 (and other integer types) as number: Atrium's ids and counters stay below 2^53 (Number.MAX_SAFE_INTEGER).
    // In Specta v2 with tauri-specta, this mirrors BigIntExportBehavior::Number via Remapper rules.
    let number = <specta_typescript::Number as Type>::definition(&mut specta::Types::default());
    let remapper = Remapper::new()
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::usize),
            number.clone(),
        )
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::isize),
            number.clone(),
        )
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::u64),
            number.clone(),
        )
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::i64),
            number.clone(),
        )
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::u128),
            number.clone(),
        )
        .rule(
            specta::datatype::DataType::Primitive(specta::datatype::Primitive::i128),
            number.clone(),
        )
        .rule(
            <specta_typescript::BigInt as Type>::definition(&mut specta::Types::default()),
            number,
        );

    struct RemapFormat(Remapper);
    impl specta::Format for RemapFormat {
        fn map_types(
            &'_ self,
            types: &specta::Types,
        ) -> Result<std::borrow::Cow<'_, specta::Types>, specta::FormatError> {
            let types = specta_serde::Format.map_types(types)?;
            Ok(std::borrow::Cow::Owned(
                self.0.remap_types(types.into_owned()),
            ))
        }
        fn map_type(
            &'_ self,
            types: &specta::Types,
            dt: &specta::datatype::DataType,
        ) -> Result<std::borrow::Cow<'_, specta::datatype::DataType>, specta::FormatError> {
            let dt = specta_serde::Format.map_type(types, dt)?;
            Ok(std::borrow::Cow::Owned(self.0.remap_dt(dt.into_owned())))
        }
    }

    let out_path = if let Some(arg) = std::env::args().nth(1) {
        PathBuf::from(arg)
    } else {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("bindings").join("protocol.ts")
    };

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).expect("failed to create output parent directory");
    }

    specta_typescript::Typescript::default()
        .export_to(&out_path, &types, RemapFormat(remapper))
        .expect("failed to export TypeScript definitions");

    println!("Wrote protocol bindings to {}", out_path.display());
}

#[cfg(not(feature = "ts"))]
fn main() {
    eprintln!("Feature 'ts' must be enabled to run this example.");
    std::process::exit(1);
}
