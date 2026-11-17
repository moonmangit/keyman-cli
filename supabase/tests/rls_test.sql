begin;
create extension if not exists pgtap with schema extensions;
select plan(71);

-- Users: A (owner), B (invitee/member), C (unrelated), D (email not confirmed)
insert into auth.users (id, email, email_confirmed_at) values
  ('aaaaaaaa-0000-0000-0000-00000000000a', 'a@example.com', now()),
  ('bbbbbbbb-0000-0000-0000-00000000000b', 'b@example.com', now()),
  ('cccccccc-0000-0000-0000-00000000000c', 'c@example.com', now());
insert into auth.users (id, email) values ('dddddddd-0000-0000-0000-00000000000d', 'd@example.com');
insert into public.profiles (user_id, email, public_key, kdf_salt, kdf_params, private_key_pw, private_key_recovery) values
  ('bbbbbbbb-0000-0000-0000-00000000000b', 'b@example.com', 'pk', 's', '{}', 'pw', 'rec'),
  ('cccccccc-0000-0000-0000-00000000000c', 'c@example.com', 'pk', 's', '{}', 'pw', 'rec');

-- ===== A =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"aaaaaaaa-0000-0000-0000-00000000000a","email":"a@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select throws_ok(
  $$ insert into public.profiles (user_id, email, public_key, kdf_salt, kdf_params, private_key_pw, private_key_recovery)
     values ('aaaaaaaa-0000-0000-0000-00000000000a', 'b@example.com', 'pk', 's', '{}', 'pw', 'rec') $$,
  '42501', null, 'profile_insert_requires_own_email');
select lives_ok(
  $$ insert into public.profiles (user_id, email, public_key, kdf_salt, kdf_params, private_key_pw, private_key_recovery)
     values ('aaaaaaaa-0000-0000-0000-00000000000a', 'a@example.com', 'pk', 's', '{}', 'pw', 'rec') $$,
  'profile insert with own verified email works');
select throws_ok(
  $$ update public.profiles set email = 'other@example.com' where user_id = 'aaaaaaaa-0000-0000-0000-00000000000a' $$,
  '42501', null, 'profile_update_requires_verified_email');

do $$ begin perform set_config('t.v', public.create_vault('group', 'x', 'k')::text, true); end $$;
select is(
  (select role || ':' || key_generation from public.vault_members
    where vault_id = current_setting('t.v')::uuid and user_id = 'aaaaaaaa-0000-0000-0000-00000000000a'),
  'owner:1', 'create_vault_makes_owner_member');

select lives_ok($$ select public.create_vault('personal', 'x', 'k') $$, 'first personal vault works');
select throws_ok($$ select public.create_vault('personal', 'x', 'k') $$, '23505', null, 'second_personal_vault_refused');

-- direct writes to server-owned tables are refused
select throws_ok(
  $$ insert into public.vaults (kind, owner_id) values ('group', 'aaaaaaaa-0000-0000-0000-00000000000a') $$,
  '42501', null, 'client cannot insert vaults directly');
select throws_ok(
  $$ insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
     values (current_setting('t.v')::uuid, 'cccccccc-0000-0000-0000-00000000000c', 'member', 'k', 1) $$,
  '42501', null, 'client cannot insert vault_members directly');
select throws_ok(
  $$ update public.vaults set key_generation = 50 where id = current_setting('t.v')::uuid $$,
  '42501', null, 'client cannot update vaults directly');
select throws_ok(
  $$ update public.vault_members set role = 'owner' where vault_id = current_setting('t.v')::uuid $$,
  '42501', null, 'client cannot update vault_members directly');

-- the owner may change the sealed name of a group, and only that column
select lives_ok(
  $$ update public.vaults set name_sealed = 'x2' where id = current_setting('t.v')::uuid $$,
  'owner_sets_name_only (owner sets the name)');
select is(
  (select name_sealed from public.vaults where id = current_setting('t.v')::uuid), 'x2',
  'owner_sets_name_only (the name changed)');
select throws_ok(
  $$ update public.vaults set key_generation = 7 where id = current_setting('t.v')::uuid $$,
  '42501', null, 'owner_sets_name_only (owner cannot update key_generation)');
select throws_ok(
  $$ update public.vaults set owner_id = 'cccccccc-0000-0000-0000-00000000000c' where id = current_setting('t.v')::uuid $$,
  '42501', null, 'owner_sets_name_only (owner cannot update owner_id)');
do $$ declare n int; begin
  update public.vaults set name_sealed = 'p' where owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and kind = 'personal';
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'owner_sets_name_only (owner cannot rename a personal vault)');

-- A adds two projects and invites B and D
do $$ begin
  perform set_config('t.p1', gen_random_uuid()::text, true);
  perform set_config('t.p2', gen_random_uuid()::text, true);
  insert into public.projects (id, vault_id, key_generation, sealed, updated_by) values
    (current_setting('t.p1')::uuid, current_setting('t.v')::uuid, 1, 's1', 'aaaaaaaa-0000-0000-0000-00000000000a'),
    (current_setting('t.p2')::uuid, current_setting('t.v')::uuid, 1, 's2', 'aaaaaaaa-0000-0000-0000-00000000000a');
  perform set_config('t.inv', gen_random_uuid()::text, true);
  insert into public.invites (id, vault_id, email, invited_by)
    values (current_setting('t.inv')::uuid, current_setting('t.v')::uuid, 'b@example.com', 'aaaaaaaa-0000-0000-0000-00000000000a');
  insert into public.invites (vault_id, email, invited_by)
    values (current_setting('t.v')::uuid, 'd@example.com', 'aaaaaaaa-0000-0000-0000-00000000000a');
end $$;

select is(
  (select count(*)::int from public.profiles where email = 'b@example.com'), 1,
  'member_reads_invitee_profile (A sees B via invite)');
select throws_ok(
  $$ insert into public.invites (vault_id, email, invited_by)
     values ((select id from public.vaults where owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and kind = 'personal'),
             'c@example.com', 'aaaaaaaa-0000-0000-0000-00000000000a') $$,
  '42501', null, 'invite_to_personal_vault_refused');
select throws_ok(
  $$ update public.projects
       set vault_id = (select id from public.vaults where owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and kind = 'personal')
     where id = current_setting('t.p1')::uuid $$,
  'P0001', 'a project cannot move to a different vault', 'project_cannot_move_to_another_vault');
select is((select email from public.profiles where user_id = 'aaaaaaaa-0000-0000-0000-00000000000a'), 'a@example.com',
  'own_public_columns_readable');
select is((select (public.my_profile()).private_key_pw), 'pw', 'my_profile_returns_own_row');
select throws_ok(
  $$ select private_key_pw from public.profiles where user_id = 'bbbbbbbb-0000-0000-0000-00000000000b' $$,
  '42501', null, 'member_or_inviter_cannot_read_private_key_columns');

-- ===== D (signed in, email never confirmed; the JWT claims an email) =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"dddddddd-0000-0000-0000-00000000000d","email":"d@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select throws_ok(
  $$ insert into public.profiles (user_id, email, public_key, kdf_salt, kdf_params, private_key_pw, private_key_recovery)
     values ('dddddddd-0000-0000-0000-00000000000d', 'd@example.com', 'pk', 's', '{}', 'pw', 'rec') $$,
  '42501', null, 'unconfirmed_user_cannot_insert_profile');
select is((select count(*)::int from public.invites), 0, 'unconfirmed_user_cannot_see_invites_to_their_email');

-- ===== B (not yet a member) =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"bbbbbbbb-0000-0000-0000-00000000000b","email":"b@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select is(
  (select count(*)::int from public.projects where vault_id = current_setting('t.v')::uuid), 0,
  'non_member_cannot_read_projects');
select is((select count(*)::int from public.invites), 1, 'invitee_sees_own_invite');

-- ===== C (no relation) =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"cccccccc-0000-0000-0000-00000000000c","email":"c@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select is(
  (select count(*)::int from public.profiles where user_id = 'aaaaaaaa-0000-0000-0000-00000000000a'), 0,
  'member_reads_invitee_profile (C cannot see A)');
select throws_ok(
  $$ select private_key_pw from public.profiles where user_id = 'aaaaaaaa-0000-0000-0000-00000000000a' $$,
  '42501', null, 'stranger_cannot_read_private_key_columns');
do $$ declare n int; begin
  update public.projects set sealed = 'x' where id = current_setting('t.p1')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'non_member_update_changes_0_rows');

do $$ declare n int; begin
  delete from public.projects where id = current_setting('t.p1')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'non_member_delete_changes_0_rows');

-- ===== A accepts =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"aaaaaaaa-0000-0000-0000-00000000000a","email":"a@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select throws_ok(
  $$ select public.accept_invite(current_setting('t.inv')::uuid, 'cccccccc-0000-0000-0000-00000000000c', 'k', 1) $$,
  'P0001', 'the user does not match the invite email', 'accept_invite_needs_matching_email');
select lives_ok(
  $$ select public.accept_invite(current_setting('t.inv')::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b', 'kb', 1) $$,
  'accept_invite_by_member runs');
reset role;
select is(
  (select count(*)::int from public.vault_members where vault_id = current_setting('t.v')::uuid and user_id = 'bbbbbbbb-0000-0000-0000-00000000000b' and role = 'member' and wrapped_key = 'kb'),
  1, 'accept_invite_by_member: B is a member');
select is(
  (select count(*)::int from public.invites where id = current_setting('t.inv')::uuid),
  0, 'accept_invite_by_member: invite row gone');

-- ===== B (now a member) =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"bbbbbbbb-0000-0000-0000-00000000000b","email":"b@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select throws_ok(
  $$ insert into public.invites (vault_id, email, invited_by)
     values (current_setting('t.v')::uuid, 'c@example.com', 'bbbbbbbb-0000-0000-0000-00000000000b') $$,
  '42501', null, 'only_owner_invites');

do $$ declare n int; begin
  update public.projects set sealed = 'stale', updated_by = 'bbbbbbbb-0000-0000-0000-00000000000b' where id = current_setting('t.p1')::uuid and version = 0;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'push_requires_current_version (stale filter changes 0 rows)');

do $$ declare n int; begin
  update public.projects set sealed = 's1b', version = 2, updated_by = 'bbbbbbbb-0000-0000-0000-00000000000b'
    where id = current_setting('t.p1')::uuid and version = 1;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 1, 'push_requires_current_version (current filter changes 1 row)');
select is((select version from public.projects where id = current_setting('t.p1')::uuid), 2,
  'push_requires_current_version (version is 2)');

do $$ begin
  update public.projects set sealed = 's1c', version = 10, updated_by = 'bbbbbbbb-0000-0000-0000-00000000000b'
    where id = current_setting('t.p1')::uuid;
end $$;
select is((select version from public.projects where id = current_setting('t.p1')::uuid), 3,
  'version_trigger_forces_increment');

do $$ begin
  update public.projects set sealed = 's1d', updated_by = 'aaaaaaaa-0000-0000-0000-00000000000a'
    where id = current_setting('t.p1')::uuid;
end $$;
select is(
  (select updated_by::text || ':' || version from public.projects where id = current_setting('t.p1')::uuid),
  'bbbbbbbb-0000-0000-0000-00000000000b:4', 'spoofed_updated_by_is_overwritten');

select throws_ok(
  $$ update public.projects set key_generation = 99 where id = current_setting('t.p1')::uuid $$,
  'P0001', 'the key generation is out of date', 'stale_key_generation_update_refused');
select throws_ok(
  $$ insert into public.projects (vault_id, key_generation, sealed, updated_by)
     values (current_setting('t.v')::uuid, 99, 's', 'bbbbbbbb-0000-0000-0000-00000000000b') $$,
  'P0001', 'the key generation is out of date', 'insert_requires_current_generation');

do $$ declare n int; begin
  delete from public.vaults where id = current_setting('t.v')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'vaults_delete refuses a non-owner member');

do $$ declare n int; begin
  update public.vaults set name_sealed = 'hijack' where id = current_setting('t.v')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'owner_sets_name_only (a member who is not the owner updates 0 rows)');

select throws_ok(
  $$ select public.rotate_vault(current_setting('t.v')::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"}]'::jsonb, '[]'::jsonb, null) $$,
  'P0001', 'only the owner can remove a member', 'rotate_requires_owner');

-- ===== A rotates =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"aaaaaaaa-0000-0000-0000-00000000000a","email":"a@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select throws_ok(
  $$ select public.rotate_vault(current_setting('t.v')::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[]'::jsonb, '[]'::jsonb, null) $$,
  'P0001', 'a new key is necessary for each remaining member', 'rotate_requires_all_members_and_projects (member key missing)');
select throws_ok(
  format($$ select public.rotate_vault(%L::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"}]'::jsonb,
       '[{"id":"%s","version":4,"sealed":"n1"}]'::jsonb, null) $$, current_setting('t.v'), current_setting('t.p1')),
  'P0001', 'all projects of the vault must be encrypted again', 'rotate_requires_all_members_and_projects (project missing)');
select throws_ok(
  format($$ select public.rotate_vault(%L::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"}]'::jsonb,
       '[{"id":"%s","version":1,"sealed":"n1"},{"id":"%s","version":1,"sealed":"n2"}]'::jsonb, null) $$,
       current_setting('t.v'), current_setting('t.p1'), current_setting('t.p2')),
  'P0001', 'a project changed during the removal; try again', 'rotate_refuses_stale_version');
select throws_ok(
  format($$ select public.rotate_vault(%L::uuid, 'aaaaaaaa-0000-0000-0000-00000000000a',
       '[{"user_id":"bbbbbbbb-0000-0000-0000-00000000000b","wrapped_key":"k2"}]'::jsonb, '[]'::jsonb, null) $$,
       current_setting('t.v')),
  'P0001', 'the owner cannot be removed', 'cannot_remove_owner');
select throws_ok(
  format($$ select public.rotate_vault(%L::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"}]'::jsonb,
       '[{"id":"%s","version":4,"sealed":"n1"},{"id":"%s","version":5,"sealed":"n1b"}]'::jsonb, null) $$,
       current_setting('t.v'), current_setting('t.p1'), current_setting('t.p1')),
  'P0001', 'all projects of the vault must be encrypted again', 'rotate_refuses_duplicate_projects');

select is(
  (select public.rotate_vault(current_setting('t.v')::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"}]'::jsonb,
       jsonb_build_array(
         jsonb_build_object('id', current_setting('t.p1'), 'version', 4, 'sealed', 'n1'),
         jsonb_build_object('id', current_setting('t.p2'), 'version', 1, 'sealed', 'n2')),
       'name2')),
  2, 'rotate_removes_member (returns generation 2)');

-- unfiltered truth, read as postgres
reset role;
select is(
  (select count(*)::int from public.vault_members where vault_id = current_setting('t.v')::uuid and user_id = 'bbbbbbbb-0000-0000-0000-00000000000b'),
  0, 'rotate_removes_member (B has no row)');
select is(
  (select key_generation from public.vaults where id = current_setting('t.v')::uuid), 2,
  'rotate_removes_member (vault generation 2)');
select is(
  (select string_agg(version::text || ':' || key_generation, ',' order by sealed) from public.projects where vault_id = current_setting('t.v')::uuid),
  '5:2,2:2', 'rotate_removes_member (projects incremented, re-sealed at generation 2)');
select is(
  (select key_generation from public.vault_members where vault_id = current_setting('t.v')::uuid and user_id = 'aaaaaaaa-0000-0000-0000-00000000000a'),
  2, 'rotate_removes_member (owner key at generation 2)');
select is(
  (select name_sealed from public.vaults where id = current_setting('t.v')::uuid), 'name2',
  'rotate_vault writes the re-sealed vault name');

-- ===== B (removed) =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"bbbbbbbb-0000-0000-0000-00000000000b","email":"b@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

select is(
  (select count(*)::int from public.projects where vault_id = current_setting('t.v')::uuid), 0,
  'rotate_removes_member (B selects projects -> 0 rows)');
select throws_ok(
  $$ insert into public.projects (vault_id, key_generation, sealed, updated_by)
     values (current_setting('t.v')::uuid, 2, 's', 'bbbbbbbb-0000-0000-0000-00000000000b') $$,
  'P0001', 'not a member of this vault', 'removed_member_insert_refused');
do $$ declare n int; begin
  update public.projects set sealed = 'late' where id = current_setting('t.p1')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'removed_member_update_changes_0_rows');

-- ===== A again =====
reset role;
do $$ begin perform set_config('request.jwt.claims', '{"sub":"aaaaaaaa-0000-0000-0000-00000000000a","email":"a@example.com","role":"authenticated"}', true); end $$;
set local role authenticated;

do $$ begin
  perform set_config('t.invc', gen_random_uuid()::text, true);
  insert into public.invites (id, vault_id, email, invited_by)
    values (current_setting('t.invc')::uuid, current_setting('t.v')::uuid, 'c@example.com', 'aaaaaaaa-0000-0000-0000-00000000000a');
end $$;
select throws_ok(
  $$ select public.accept_invite(current_setting('t.invc')::uuid, 'cccccccc-0000-0000-0000-00000000000c', 'k', 1) $$,
  'P0001', 'the group key changed; run the command again', 'accept_invite_refuses_stale_generation');

do $$ declare n int; begin
  delete from public.vaults where owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and kind = 'personal';
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 0, 'vaults_delete refuses a personal vault');

do $$ declare n int; begin
  perform set_config('t.v3', public.create_vault('group', 'z', 'k')::text, true);
  delete from public.vaults where id = current_setting('t.v3')::uuid;
  get diagnostics n = row_count; perform set_config('t.n', n::text, true);
end $$;
select is(current_setting('t.n')::int, 1, 'vaults_delete lets the owner delete a group vault');

-- ===== duplicate-key hole: keys [A, A] must not leave B on a stale key =====
do $$ begin perform set_config('t.v2', public.create_vault('group', 'y', 'k')::text, true); end $$;
reset role;
-- add B and C as members of V2 (setup as postgres; the client path is covered by accept_invite above)
insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation) values
  (current_setting('t.v2')::uuid, 'bbbbbbbb-0000-0000-0000-00000000000b', 'member', 'kb', 1),
  (current_setting('t.v2')::uuid, 'cccccccc-0000-0000-0000-00000000000c', 'member', 'kc', 1);
set local role authenticated;
select throws_ok(
  format($$ select public.rotate_vault(%L::uuid, 'cccccccc-0000-0000-0000-00000000000c',
       '[{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k2"},{"user_id":"aaaaaaaa-0000-0000-0000-00000000000a","wrapped_key":"k3"}]'::jsonb,
       '[]'::jsonb, null) $$, current_setting('t.v2')),
  'P0001', 'every remaining member must get a new key', 'rotate_refuses_duplicate_keys');

-- ===== invites only work for group vaults (invite planted as postgres; clients cannot insert one) =====
reset role;
insert into public.invites (vault_id, email, invited_by)
  select id, 'c@example.com', owner_id from public.vaults
  where owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and kind = 'personal';
set local role authenticated;
select throws_ok(
  $$ select public.accept_invite(
       (select i.id from public.invites i join public.vaults v on v.id = i.vault_id
         where v.owner_id = 'aaaaaaaa-0000-0000-0000-00000000000a' and v.kind = 'personal'),
       'cccccccc-0000-0000-0000-00000000000c', 'k', 1) $$,
  'P0001', 'only a group accepts invites', 'accept_invite_to_personal_vault_refused');

-- ===== emails must be lowercase (as postgres) =====
reset role;
select throws_ok(
  $$ insert into public.profiles (user_id, email, public_key, kdf_salt, kdf_params, private_key_pw, private_key_recovery)
     values ('dddddddd-0000-0000-0000-00000000000d', 'Up@Example.com', 'pk', 's', '{}', 'pw', 'rec') $$,
  '23514', null, 'profile_email_must_be_lowercase');
select throws_ok(
  $$ insert into public.invites (vault_id, email, invited_by)
     values (current_setting('t.v')::uuid, 'Up@Example.com', 'aaaaaaaa-0000-0000-0000-00000000000a') $$,
  '23514', null, 'invite_email_must_be_lowercase');

-- ===== anon: nothing =====
set local role anon;
select throws_ok($$ select * from public.profiles $$, '42501', null, 'anon_denied_profiles');
select throws_ok($$ select * from public.vaults $$, '42501', null, 'anon_denied_vaults');
select throws_ok($$ select * from public.vault_members $$, '42501', null, 'anon_denied_vault_members');
select throws_ok($$ select * from public.invites $$, '42501', null, 'anon_denied_invites');
select throws_ok($$ select * from public.projects $$, '42501', null, 'anon_denied_projects');
select throws_ok($$ select public.create_vault('group', 'x', 'k') $$, '42501', null, 'anon_denied_create_vault');

reset role;
select * from finish();
rollback;
