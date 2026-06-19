//! check.rs 单测（文件 ≤400 行纪律：从 check.rs 拆出，经 #[path] 挂回）。
use super::*;
use serde_json::json;
use trpg_model::{CheckTargetModel, RollAuthority, RollVisibility};

#[test]
fn roll_check_without_mechanic_still_requires_tested_parameter() {
    let err =
        parse_roll_check_args(json!({"check_label":"Scan","visibility":"public"})).unwrap_err();
    assert!(err.to_string().contains("invalid_arguments"));
}

#[test]
fn mechanic_inheritance_fills_tested_parameter_and_dice() {
    let entry = MechanicEntry {
        id: "coc.skill.jump".to_string(),
        name: "Jump".to_string(),
        tested_parameter: Some("jump".to_string()),
        procedure: vec![ProcedureStep::Roll {
            dice: "1d100".to_string(),
            vs: None,
            note: None,
        }],
        ..Default::default()
    };
    let mut args = RollCheckArgs {
        check_label: "Jump the chasm".to_string(),
        tested_parameter: String::new(),
        actor_id: None,
        opposed: None,
        visibility: "public".to_string(),
        intent_kind: None,
        mechanic_id: Some("coc.skill.jump".to_string()),
        scene_mechanic_id: None,
    };
    let dice = apply_mechanic_inheritance(&mut args, &entry);
    assert_eq!(args.tested_parameter, "jump");
    assert_eq!(dice, Some("1d100".to_string()));
}

// Phase3 §4.1：GM 漏填 opposed 时，对抗预 pass 的 binding 被注入；显式 opposed
// 优先（GM 自己填的不被覆盖）；binding=None 时不动（fail-closed 退回单方检定）。
#[test]
fn prepass_binding_injected_when_gm_omits_opposed() {
    use crate::opposed_prepass::OpposedBinding;
    use trpg_runtime::npc_synth::NpcPersona;
    let binding = OpposedBinding {
        persona: NpcPersona {
            actor_id: "npc.scav_boss".into(),
            name: "Scav Boss".into(),
            prose: "".into(),
        },
        bucket: "stats".into(),
        opponent_parameter: "defense".into(),
    };
    // GM 漏填 opposed → 注入真实场景 NPC id + 防御键。
    let mut args = RollCheckArgs {
        check_label: "Handgun attack".into(),
        tested_parameter: "Handgun".into(),
        actor_id: None,
        opposed: None,
        visibility: "public".into(),
        intent_kind: None,
        mechanic_id: None,
        scene_mechanic_id: None,
    };
    inject_opposed_binding(&mut args, Some(&binding));
    let opp = args.opposed.expect("binding must be injected");
    assert_eq!(opp.npc_id, "npc.scav_boss");
    assert_eq!(opp.opponent_parameter, "defense");
    assert_eq!(opp.bucket, "stats");
    // 显式 opposed 优先：GM 自己填了就不被预 pass 覆盖。
    let mut explicit = RollCheckArgs {
        check_label: "x".into(),
        tested_parameter: "Handgun".into(),
        actor_id: None,
        opposed: Some(OpposedArgs {
            npc_id: "npc.explicit".into(),
            opponent_parameter: "evasion".into(),
            bucket: "skills".into(),
        }),
        visibility: "public".into(),
        intent_kind: None,
        mechanic_id: None,
        scene_mechanic_id: None,
    };
    inject_opposed_binding(&mut explicit, Some(&binding));
    assert_eq!(
        explicit.opposed.unwrap().npc_id,
        "npc.explicit",
        "explicit GM opposed must win over prepass binding"
    );
    // fail-closed：无 binding → 不动（退回单方检定）。
    let mut bare = RollCheckArgs {
        check_label: "x".into(),
        tested_parameter: "Handgun".into(),
        actor_id: None,
        opposed: None,
        visibility: "public".into(),
        intent_kind: None,
        mechanic_id: None,
        scene_mechanic_id: None,
    };
    inject_opposed_binding(&mut bare, None);
    assert!(
        bare.opposed.is_none(),
        "no binding → no injection (fail-closed)"
    );
}

// 兜底攻击目标绑定（live e2e session_codex_e2e_20260619_05 回归）：GM 漏填
// opposed/显式目标的真实玩家火器命中骰必须绑定透明兜底目标 npc.opposition，
// 否则 record_attack 因无目标 fail-closed，零伤害落账（attack_resolution 0 行）。
fn attack_args(label: &str, mechanic: Option<&str>, actor: Option<&str>) -> RollCheckArgs {
    RollCheckArgs {
        check_label: label.to_string(),
        tested_parameter: "Firearms (Handgun)".to_string(),
        actor_id: actor.map(str::to_string),
        opposed: None,
        visibility: "public".to_string(),
        intent_kind: None,
        mechanic_id: mechanic.map(str::to_string),
        scene_mechanic_id: None,
    }
}

#[test]
fn ranged_firearm_without_opposed_binds_fallback_opposition_target() {
    // 失败证据原样 label + mechanic：玩家用 M1911 开火，GM 没填 opposed/目标。
    let args = attack_args(
        "Evelyn fires her Colt M1911 at the knife-wielding attacker",
        Some("call_of_cthulhu_7e.ranged_and_thrown_attack_resolution"),
        Some("pc.current"),
    );
    let target = fallback_attack_target_actor_for_roll(&args)
        .expect("genuine firearm attack must bind a fallback target");
    assert_eq!(target.actor_id, "npc.opposition");
    assert_eq!(target.actor_kind, ActorKind::Npc);
    // actor 缺省 → pc.current（玩家）：同样绑定。
    let mut missing_actor = args.clone();
    missing_actor.actor_id = None;
    assert_eq!(
        fallback_attack_target_actor_for_roll(&missing_actor)
            .expect("missing actor defaults to player → binds")
            .actor_id,
        "npc.opposition"
    );
}

#[test]
fn status_and_aftermath_labels_do_not_bind_fallback_target() {
    // 攻击的“后果/状态/查询”——含 shot/attacker 子串也绝不绑定兜底目标。
    for (label, mech) in [
        (
            "Zero HP state after gunshot",
            Some("call_of_cthulhu_7e.zero_hp_state"),
        ),
        (
            "Major wound consciousness after the shot",
            Some("call_of_cthulhu_7e.major_wound"),
        ),
        ("Status query: is the attacker down?", None),
    ] {
        let args = attack_args(label, mech, Some("pc.current"));
        assert!(
            fallback_attack_target_actor_for_roll(&args).is_none(),
            "status/aftermath label `{label}` must not bind a fallback target"
        );
    }
}

#[test]
fn non_attack_rolls_do_not_bind_fallback_target() {
    for label in [
        "Stealth past the guard",
        "Roll initiative",
        "Reload the Colt M1911",
        "Dodge the incoming knife",
        "Apply 1d10+2 damage to the target",
        "Effect roll",
    ] {
        let args = attack_args(label, None, Some("pc.current"));
        assert!(
            fallback_attack_target_actor_for_roll(&args).is_none(),
            "non-attack label `{label}` must not bind a fallback target"
        );
    }
}

#[test]
fn npc_actor_attack_without_explicit_target_does_not_bind_opposition() {
    // npc.* 发起的攻击不给通用 npc.opposition，避免自打/NPC 串话。
    let args = attack_args(
        "The cultist fires at Evelyn",
        Some("call_of_cthulhu_7e.ranged_and_thrown_attack_resolution"),
        Some("npc.opposition"),
    );
    assert!(
        fallback_attack_target_actor_for_roll(&args).is_none(),
        "npc.* attacker must not receive a generic npc.opposition fallback"
    );
}

#[test]
fn explicit_opposed_suppresses_fallback_target() {
    // 显式 opposed 自带 target（走 stamp_opposed_check），兜底不得插手/覆盖。
    let mut args = attack_args(
        "Evelyn fires her Colt M1911 at the attacker",
        Some("call_of_cthulhu_7e.ranged_and_thrown_attack_resolution"),
        Some("pc.current"),
    );
    args.opposed = Some(OpposedArgs {
        npc_id: "npc.explicit".to_string(),
        opponent_parameter: "dodge".to_string(),
        bucket: "skills".to_string(),
    });
    assert!(
        fallback_attack_target_actor_for_roll(&args).is_none(),
        "explicit opposed must suppress the unopposed fallback target"
    );
}

#[test]
fn explicit_args_beat_catalog() {
    let entry = MechanicEntry {
        id: "coc.skill.jump".to_string(),
        name: "Jump".to_string(),
        tested_parameter: Some("jump".to_string()),
        ..Default::default()
    };
    let mut args = RollCheckArgs {
        check_label: "Climb the wall".to_string(),
        tested_parameter: "climb".to_string(),
        actor_id: None,
        opposed: None,
        visibility: "public".to_string(),
        intent_kind: None,
        mechanic_id: Some("coc.skill.jump".to_string()),
        scene_mechanic_id: None,
    };
    apply_mechanic_inheritance(&mut args, &entry);
    assert_eq!(args.tested_parameter, "climb");
}

#[test]
fn build_contract_binds_tested_parameter_and_visibility() {
    let args = RollCheckArgs {
        check_label: "Stealth".to_string(),
        tested_parameter: "Stealth".to_string(),
        actor_id: Some("pc.current".to_string()),
        opposed: None,
        visibility: "secret".to_string(),
        intent_kind: Some("stealth_or_dangerous_movement".to_string()),
        mechanic_id: None,
        scene_mechanic_id: None,
    };
    let contract =
        build_check_contract_for_args("s", "t", "cyberpunk_red", Some("homecoming"), &args, "1d10")
            .unwrap();
    assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "Stealth");
    assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
    assert_eq!(contract.roll_authority, RollAuthority::System);
    // CheckTargetModel 是带数据枚举，没有 as_str()，必须用 matches! 断言。
    assert!(matches!(
        contract.target,
        CheckTargetModel::UnknownUntilLookup
    ));
}

#[test]
fn build_contract_infers_npc_actor_kind_from_actor_id() {
    let args = RollCheckArgs {
        check_label: "Major wound consciousness".to_string(),
        tested_parameter: "con".to_string(),
        actor_id: Some("npc.opposition".to_string()),
        opposed: None,
        visibility: "public".to_string(),
        intent_kind: Some("agent_selected_check".to_string()),
        mechanic_id: Some("call_of_cthulhu_7e.major_wound".to_string()),
        scene_mechanic_id: None,
    };
    let contract =
        build_check_contract_for_args("s", "t", "call_of_cthulhu_7e", None, &args, "1d100")
            .unwrap();
    assert_eq!(contract.initiator.actor_id, "npc.opposition");
    assert_eq!(contract.initiator.actor_kind, ActorKind::Npc);
    assert_eq!(
        contract.initiator.display_name.as_deref(),
        Some("npc.opposition")
    );
}

#[test]
fn mechanic_without_roll_step_is_passive_not_default_roll() {
    let entry = MechanicEntry {
        id: "call_of_cthulhu_7e.zero_hp_state".to_string(),
        name: "Zero hit points and dying".to_string(),
        tested_parameter: Some("hit_points".to_string()),
        procedure: vec![ProcedureStep::Gate {
            condition: "The character is unconscious.".to_string(),
            note: Some("This always happens at zero hit points.".to_string()),
        }],
        ..Default::default()
    };
    assert!(!mechanic_has_roll_step(&entry));
}

// Near-direct call-path coverage for the passive guard at check.rs:439-441.
// A no-Roll-step mechanic (CoC `zero_hp_state`: apply/gate only, no roll) must
// resolve via `passive_mechanic_result` — the exact value the `call()` path
// returns — proving `passive_mechanic: true`, `rolled: false`, and that the
// kernel default die (`1d100`) is never used. The live regression rolled a
// `1d100` against `hit_points` for this very mechanic.
#[test]
fn passive_mechanic_call_path_returns_no_roll_and_no_default_die() {
    let entry = MechanicEntry {
        id: "call_of_cthulhu_7e.zero_hp_state".to_string(),
        name: "Zero hit points and dying".to_string(),
        tested_parameter: Some("hit_points".to_string()),
        procedure: vec![ProcedureStep::Gate {
            condition: "The character is unconscious.".to_string(),
            note: Some("This always happens at zero hit points.".to_string()),
        }],
        ..Default::default()
    };
    // Branch guard the call path uses to pick the passive return.
    assert!(
        !mechanic_has_roll_step(&entry),
        "zero_hp_state must have no roll step"
    );
    let result = passive_mechanic_result("call_of_cthulhu_7e.zero_hp_state", &entry);
    assert_eq!(result["passive_mechanic"], json!(true));
    assert_eq!(result["rolled"], json!(false));
    assert_eq!(
        result["mechanic_id"],
        json!("call_of_cthulhu_7e.zero_hp_state")
    );
    // The kernel default die (`1d100`) must never be reached for a no-roll
    // mechanic, and no roll-step `dice` field may leak into a passive result.
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(
        !serialized.contains("1d100") && !serialized.contains("\"dice\""),
        "passive result must not carry a dice expression: {serialized}"
    );
}

// Active-NPC fail-closed precondition. The gate that fires BEFORE any roll or
// check contract is built (check.rs:471, ahead of build_check_contract at 472)
// must demand materialization for an `npc.*` actor with a tested parameter,
// routing to the correct bucket. When the gate returns Some, an unmaterialized
// parameter makes `ensure_active_npc_parameter_if_needed` fail closed with
// `npc_synthesis_unavailable` before a contract exists. (The Ok(None)->Err leg
// itself needs a live engine/DB; see handoff blocker note.)
#[test]
fn active_npc_parameter_gate_requires_materialization_before_roll() {
    // npc.* actor + tested parameter → gate fires, routed to the right bucket.
    assert_eq!(
        active_npc_parameter_gate(Some("npc.opposition"), "con"),
        Some(("npc.opposition", "stats", "con")),
        "an active npc.* roll on a CON parameter must hit the synthesis gate"
    );
    assert_eq!(
        active_npc_parameter_gate(Some("  npc.opposition  "), "hit_points"),
        Some(("npc.opposition", "resources", "hit_points")),
        "hit_points routes to resources; actor id is trimmed"
    );
    // PC actor → no active-NPC gate (PC parameters resolve from the sheet).
    assert_eq!(active_npc_parameter_gate(Some("pc.current"), "con"), None);
    // Missing or empty inputs → no gate.
    assert_eq!(active_npc_parameter_gate(None, "con"), None);
    assert_eq!(
        active_npc_parameter_gate(Some("npc.opposition"), "  "),
        None
    );
    assert_eq!(active_npc_parameter_gate(Some("   "), "con"), None);
}

// B3: roll_check 结果 JSON 的 band_semantics 渲染（纯函数级——构造
// outcome+dice_core 调 band_semantics_line；工具级走真 db 的留 C5 e2e）。
#[test]
fn roll_check_result_carries_band_semantics_when_kernel_has_it() {
    let dice_core =
        json!({"success_bands":[{"id":"hard","label":"困难成功","semantics":"超出常人的表现"}]});
    let outcome = json!({"success": true, "success_tier": "hard"});
    let line =
        band_semantics_line(&dice_core, &outcome).expect("kernel band semantics must render");
    for seg in ["hard", "困难成功", "超出常人的表现"] {
        assert!(line.contains(seg), "line must contain `{seg}`: {line}");
    }
    // 无 semantics 的 kernel -> None（call 端整键缺省，裸 band 仍在 outcome 里）
    let bare = json!({"success_bands":[{"id":"hard","label":"困难成功"}]});
    assert_eq!(band_semantics_line(&bare, &outcome), None);
}

// C4：scene_mechanic_id 字段就位 + 向后兼容（不给 → None）。C7：player 路径同入口。
#[test]
fn roll_args_accept_scene_mechanic_id() {
    let args = parse_roll_check_args(json!({"check_label":"Cut the cable","tested_parameter":"brawling","scene_mechanic_id":"x"})).unwrap();
    assert_eq!(args.scene_mechanic_id.as_deref(), Some("x"));
    let args =
        parse_roll_check_args(json!({"check_label":"Cut the cable","tested_parameter":"brawling"}))
            .unwrap();
    assert!(args.scene_mechanic_id.is_none());
    let p = parse_player_args(json!({"check_label":"c","tested_parameter":"brawling","stakes":{"before":"b","success":"s","failure":"f"},"scene_mechanic_id":"x"})).unwrap();
    assert_eq!(p.scene_mechanic_id.as_deref(), Some("x"));
}

#[test]
fn request_player_roll_contract_uses_player_authority() {
    let args = RequestPlayerRollArgs {
        check_label: "Athletics".to_string(),
        tested_parameter: "Athletics".to_string(),
        stakes: StakesArgs {
            before: "The jump is risky.".to_string(),
            success: "You land cleanly.".to_string(),
            failure: "You fall short.".to_string(),
        },
        visibility: "public".to_string(),
        scene_mechanic_id: None,
    };
    let contract = build_player_contract_for_args("s", "t", "sw2_5", None, &args, "2d6").unwrap();
    assert_eq!(contract.roll_visibility, RollVisibility::PlayerRollRequired);
    assert_eq!(contract.roll_authority, RollAuthority::Player);
    assert_eq!(contract.stakes.before_roll_public, "The jump is risky.");
}

// 桌面骰权政策守卫：system_rolls_visible 下玩家亲掷契约必须转系统代掷
// （GM agent 战斗冻结修复——gate 开出去叙事输入永不结算）；stakes 与
// provenance 标记保留。env 显式置位防外部 shell 注入 player 政策时误红。
#[test]
fn system_policy_converts_player_gate_to_system_authority() {
    std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");
    let args = RequestPlayerRollArgs {
        check_label: "Dodge".to_string(),
        tested_parameter: "dodge".to_string(),
        stakes: StakesArgs {
            before: "Claws rake at your throat.".to_string(),
            success: "You twist away.".to_string(),
            failure: "It tears into you.".to_string(),
        },
        visibility: "public".to_string(),
        scene_mechanic_id: None,
    };
    let player =
        build_player_contract_for_args("s", "t", "call_of_cthulhu_7e", None, &args, "1d100")
            .unwrap();
    let converted = convert_player_gate_to_system(&player);
    assert_eq!(converted.roll_authority, RollAuthority::System);
    assert_eq!(converted.roll_visibility, RollVisibility::PublicGmRoll);
    assert_eq!(
        converted.stakes.before_roll_public,
        "Claws rake at your throat."
    );
    assert!(
        converted
            .advice_refs
            .iter()
            .any(|r| r.contains("request_player_roll:converted:system_rolls_visible")),
        "provenance marker missing: {:?}",
        converted.advice_refs
    );
}
