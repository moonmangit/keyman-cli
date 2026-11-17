-- Fixes from the final review of cloud sync.

-- 1. The client makes the vault ID, so it can bind the wrapped vault key to the vault
--    before the server sees it. The old signature (the server made the ID) goes away.
drop function public.create_vault(text, text, text);

create function public.create_vault(p_id uuid, p_kind text, p_name_sealed text, p_wrapped_key text)
returns uuid
language plpgsql security definer set search_path = '' as $$
begin
  if auth.uid() is null then raise exception 'sign in first'; end if;
  if p_id is null then raise exception 'the vault needs an id'; end if;
  insert into public.vaults (id, kind, owner_id, name_sealed) values (p_id, p_kind, auth.uid(), p_name_sealed);
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (p_id, auth.uid(), 'owner', p_wrapped_key, 1);
  return p_id;
end $$;

revoke execute on function public.create_vault(uuid, text, text, text) from public, anon;
grant execute on function public.create_vault(uuid, text, text, text) to authenticated;

-- 2. A NULL generation must not skip the key generation check (`<>` gives NULL).
--    The body is the same as before except for `is distinct from`.
create or replace function public.accept_invite(p_invite uuid, p_user uuid, p_wrapped_key text, p_generation int) returns void
language plpgsql security definer set search_path = '' as $$
declare inv public.invites; gen int;
begin
  select * into inv from public.invites where id = p_invite for update;
  if not found then raise exception 'the invite does not exist'; end if;
  if not exists (select 1 from public.vaults where id = inv.vault_id and kind = 'group') then
    raise exception 'only a group accepts invites';
  end if;
  if not exists (select 1 from public.vault_members where vault_id = inv.vault_id and user_id = auth.uid()) then
    raise exception 'only a member can accept an invite';
  end if;
  if not exists (select 1 from public.profiles where user_id = p_user and email = inv.email) then
    raise exception 'the user does not match the invite email';
  end if;
  select key_generation into gen from public.vaults where id = inv.vault_id for update;
  if p_generation is distinct from gen then raise exception 'the group key changed; run the command again'; end if;
  if not exists (select 1 from public.vault_members where vault_id = inv.vault_id and user_id = auth.uid()) then
    raise exception 'only a member can accept an invite';
  end if;
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (inv.vault_id, p_user, 'member', p_wrapped_key, gen);
  delete from public.invites where id = p_invite;
end $$;

-- 3. The public key of a profile never changes after the insert. Two computers that set up
--    one account at the same time cannot replace the key pair, and members can trust the
--    key that they wrapped vault keys for.
create function private.profiles_keep_public_key() returns trigger
language plpgsql set search_path = '' as $$
begin
  if new.public_key is distinct from old.public_key then
    raise exception 'the public key cannot change';
  end if;
  return new;
end $$;
revoke execute on function private.profiles_keep_public_key() from public, anon, authenticated;

create trigger profiles_keep_public_key before update on public.profiles
  for each row execute function private.profiles_keep_public_key();
