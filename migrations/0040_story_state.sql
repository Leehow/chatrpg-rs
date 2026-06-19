-- 0040: story_state table — StoryState 持久化（P5.5 layered-runtime）。
-- 设计4补充 §11.2-§11.4 + §二-④：Director 只「提议」整盘 StoryState，runtime（System Kernel）
-- 才「提交」。本表是 runtime 落 StoryState 快照的单行存储；Director core 仍是 commit-nothing。
--
-- 单行/会话：story_state 按 session 唯一（session_id primary key），整盘 StoryState 序列化为
-- 一块 jsonb（state_json）。一个 session 只持有「当前」story 快照——同一 packet 重放 ⇒ 同一行
-- （upsert on conflict(session_id)），不堆积历史。历史 beats 在 StoryState.recent_beats 里，
-- 不另开行。
--
-- 失败软（fail-soft）：state_json 默认 '{}'::jsonb——StoryState 每字段 `#[serde(default)]`，
-- 空对象反序列化为空 story（fail-closed：不凭空造线程/压力）。读路径 load_story_state 解析失败
-- ⇒ log + None（Director 走 fallback/空 story 路径），永不 panic。
--
-- session_id `references sessions(session_id) on delete cascade`（镜像 0039 world_facts /
-- memory_facts）：session 删除时 story_state 行随之清理，不留孤儿。本表 P5.5 新建从未上线，
-- 加 FK = 纯加性硬化，非破坏性迁移（无 hard-stop / 无回填风险）。
--
-- 幂等可重放：create table if not exists + upsert on conflict(session_id)。
create table if not exists story_state (
  session_id text primary key references sessions(session_id) on delete cascade,
  state_json jsonb not null default '{}'::jsonb,
  updated_turn text
);
