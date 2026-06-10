-- Module scene navigation: track the party's current module scene per session.
alter table sessions add column if not exists current_scene_id text;
