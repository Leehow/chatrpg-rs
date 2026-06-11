# gm_skill 数据包版本化副本

运行时真身经 `data/` symlink 落在共享数据根（git 不跟踪 symlink 内容）。本目录是 **gm_skill 全树（global 准则 + ruleset 覆盖 + modes 姿态包）的版本化镜像**，三期起为权威修改入口：改这里 → 同步到共享数据根（`cp -R gm_skill_src/ $(readlink data)/agent/gm_skill/` 反向）。
