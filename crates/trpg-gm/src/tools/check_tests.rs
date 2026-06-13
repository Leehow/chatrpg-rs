//! check.rs 单测（文件 ≤400 行纪律：从 check.rs 拆出，经 #[path] 挂回）。
use super::*;
use serde_json::json;
use trpg_model::{CheckTargetModel, RollAuthority, RollVisibility};

    #[test]
    fn roll_check_without_mechanic_still_requires_tested_parameter() {
        let err = parse_roll_check_args(json!({"check_label":"Scan","visibility":"public"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn mechanic_inheritance_fills_tested_parameter_and_dice() {
        let entry = MechanicEntry {
            id: "coc.skill.jump".to_string(),
            name: "Jump".to_string(),
            tested_parameter: Some("jump".to_string()),
            procedure: vec![ProcedureStep::Roll { dice: "1d100".to_string(), vs: None, note: None }],
            ..Default::default()
        };
        let mut args = RollCheckArgs { check_label: "Jump the chasm".to_string(), tested_parameter: String::new(), actor_id: None, opposed: None, visibility: "public".to_string(), intent_kind: None, mechanic_id: Some("coc.skill.jump".to_string()), scene_mechanic_id: None };
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
            persona: NpcPersona { actor_id: "npc.scav_boss".into(), name: "Scav Boss".into(), prose: "".into() },
            bucket: "stats".into(),
            opponent_parameter: "defense".into(),
        };
        // GM 漏填 opposed → 注入真实场景 NPC id + 防御键。
        let mut args = RollCheckArgs { check_label: "Handgun attack".into(), tested_parameter: "Handgun".into(), actor_id: None, opposed: None, visibility: "public".into(), intent_kind: None, mechanic_id: None, scene_mechanic_id: None };
        inject_opposed_binding(&mut args, Some(&binding));
        let opp = args.opposed.expect("binding must be injected");
        assert_eq!(opp.npc_id, "npc.scav_boss");
        assert_eq!(opp.opponent_parameter, "defense");
        assert_eq!(opp.bucket, "stats");
        // 显式 opposed 优先：GM 自己填了就不被预 pass 覆盖。
        let mut explicit = RollCheckArgs { check_label: "x".into(), tested_parameter: "Handgun".into(), actor_id: None, opposed: Some(OpposedArgs { npc_id: "npc.explicit".into(), opponent_parameter: "evasion".into(), bucket: "skills".into() }), visibility: "public".into(), intent_kind: None, mechanic_id: None, scene_mechanic_id: None };
        inject_opposed_binding(&mut explicit, Some(&binding));
        assert_eq!(explicit.opposed.unwrap().npc_id, "npc.explicit", "explicit GM opposed must win over prepass binding");
        // fail-closed：无 binding → 不动（退回单方检定）。
        let mut bare = RollCheckArgs { check_label: "x".into(), tested_parameter: "Handgun".into(), actor_id: None, opposed: None, visibility: "public".into(), intent_kind: None, mechanic_id: None, scene_mechanic_id: None };
        inject_opposed_binding(&mut bare, None);
        assert!(bare.opposed.is_none(), "no binding → no injection (fail-closed)");
    }

    #[test]
    fn explicit_args_beat_catalog() {
        let entry = MechanicEntry {
            id: "coc.skill.jump".to_string(),
            name: "Jump".to_string(),
            tested_parameter: Some("jump".to_string()),
            ..Default::default()
        };
        let mut args = RollCheckArgs { check_label: "Climb the wall".to_string(), tested_parameter: "climb".to_string(), actor_id: None, opposed: None, visibility: "public".to_string(), intent_kind: None, mechanic_id: Some("coc.skill.jump".to_string()), scene_mechanic_id: None };
        apply_mechanic_inheritance(&mut args, &entry);
        assert_eq!(args.tested_parameter, "climb");
    }

    #[test]
    fn build_contract_binds_tested_parameter_and_visibility() {
        let args = RollCheckArgs { check_label: "Stealth".to_string(), tested_parameter: "Stealth".to_string(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: "secret".to_string(), intent_kind: Some("stealth_or_dangerous_movement".to_string()), mechanic_id: None, scene_mechanic_id: None };
        let contract = build_check_contract_for_args("s", "t", "cyberpunk_red", Some("homecoming"), &args, "1d10").unwrap();
        assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "Stealth");
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        // CheckTargetModel 是带数据枚举，没有 as_str()，必须用 matches! 断言。
        assert!(matches!(contract.target, CheckTargetModel::UnknownUntilLookup));
    }

    // B3: roll_check 结果 JSON 的 band_semantics 渲染（纯函数级——构造
    // outcome+dice_core 调 band_semantics_line；工具级走真 db 的留 C5 e2e）。
    #[test]
    fn roll_check_result_carries_band_semantics_when_kernel_has_it() {
        let dice_core = json!({"success_bands":[{"id":"hard","label":"困难成功","semantics":"超出常人的表现"}]});
        let outcome = json!({"success": true, "success_tier": "hard"});
        let line = band_semantics_line(&dice_core, &outcome).expect("kernel band semantics must render");
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
        let args = parse_roll_check_args(json!({"check_label":"Cut the cable","tested_parameter":"brawling"})).unwrap();
        assert!(args.scene_mechanic_id.is_none());
        let p = parse_player_args(json!({"check_label":"c","tested_parameter":"brawling","stakes":{"before":"b","success":"s","failure":"f"},"scene_mechanic_id":"x"})).unwrap();
        assert_eq!(p.scene_mechanic_id.as_deref(), Some("x"));
    }

    #[test]
    fn request_player_roll_contract_uses_player_authority() {
        let args = RequestPlayerRollArgs { check_label: "Athletics".to_string(), tested_parameter: "Athletics".to_string(), stakes: StakesArgs { before: "The jump is risky.".to_string(), success: "You land cleanly.".to_string(), failure: "You fall short.".to_string() }, visibility: "public".to_string(), scene_mechanic_id: None };
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
        let args = RequestPlayerRollArgs { check_label: "Dodge".to_string(), tested_parameter: "dodge".to_string(), stakes: StakesArgs { before: "Claws rake at your throat.".to_string(), success: "You twist away.".to_string(), failure: "It tears into you.".to_string() }, visibility: "public".to_string(), scene_mechanic_id: None };
        let player = build_player_contract_for_args("s", "t", "call_of_cthulhu_7e", None, &args, "1d100").unwrap();
        let converted = convert_player_gate_to_system(&player);
        assert_eq!(converted.roll_authority, RollAuthority::System);
        assert_eq!(converted.roll_visibility, RollVisibility::PublicGmRoll);
        assert_eq!(converted.stakes.before_roll_public, "Claws rake at your throat.");
        assert!(converted.advice_refs.iter().any(|r| r.contains("request_player_roll:converted:system_rolls_visible")), "provenance marker missing: {:?}", converted.advice_refs);
    }
