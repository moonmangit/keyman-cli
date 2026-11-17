create table public.profiles (
  user_id uuid primary key references auth.users on delete cascade,
  email text not null unique,
  public_key text not null,
  kdf_salt text not null,
  kdf_params jsonb not null,
  private_key_pw text not null,
  private_key_recovery text not null,
  created_at timestamptz not null default now()
);

create table public.vaults (
  id uuid primary key default gen_random_uuid(),
  kind text not null check (kind in ('personal', 'group')),
  owner_id uuid not null references auth.users on delete cascade,
  name_sealed text,
  key_generation int not null default 1,
  created_at timestamptz not null default now()
);
create unique index vaults_one_personal on public.vaults (owner_id) where kind = 'personal';

create table public.vault_members (
  vault_id uuid not null references public.vaults on delete cascade,
  user_id uuid not null references auth.users on delete cascade,
  role text not null check (role in ('owner', 'member')),
  wrapped_key text not null,
  key_generation int not null,
  primary key (vault_id, user_id)
);

create table public.invites (
  id uuid primary key default gen_random_uuid(),
  vault_id uuid not null references public.vaults on delete cascade,
  email text not null,
  invited_by uuid not null references auth.users on delete cascade,
  created_at timestamptz not null default now(),
  unique (vault_id, email)
);

create table public.projects (
  id uuid primary key default gen_random_uuid(),
  vault_id uuid not null references public.vaults on delete cascade,
  version int not null default 1,
  key_generation int not null,
  sealed text not null,
  updated_by uuid not null references auth.users,
  updated_at timestamptz not null default now()
);

alter table public.profiles enable row level security;
alter table public.vaults enable row level security;
alter table public.vault_members enable row level security;
alter table public.invites enable row level security;
alter table public.projects enable row level security;

create function public.my_vault_ids() returns setof uuid
language sql stable security definer set search_path = '' as $$
  select vault_id from public.vault_members where user_id = auth.uid()
$$;

create function public.is_vault_owner(v uuid) returns boolean
language sql stable security definer set search_path = '' as $$
  select exists (select 1 from public.vaults where id = v and owner_id = auth.uid())
$$;

create function public.is_group_owner(v uuid) returns boolean
language sql stable security definer set search_path = '' as $$
  select exists (select 1 from public.vaults where id = v and owner_id = auth.uid() and kind = 'group')
$$;

create function public.vault_generation(v uuid) returns int
language sql stable security definer set search_path = '' as $$
  select key_generation from public.vaults where id = v
$$;

-- profiles
create policy profiles_select on public.profiles for select to authenticated using (
  user_id = auth.uid()
  or user_id in (select m.user_id from public.vault_members m where m.vault_id in (select public.my_vault_ids()))
  or email in (select i.email from public.invites i where i.vault_id in (select public.my_vault_ids()))
);
create policy profiles_insert on public.profiles for insert to authenticated
  with check (user_id = auth.uid() and email = (auth.jwt() ->> 'email'));
create policy profiles_update on public.profiles for update to authenticated
  using (user_id = auth.uid())
  with check (user_id = auth.uid() and email = (auth.jwt() ->> 'email'));

-- vaults
create policy vaults_select on public.vaults for select to authenticated
  using (id in (select public.my_vault_ids()));
create policy vaults_delete on public.vaults for delete to authenticated
  using (owner_id = auth.uid() and kind = 'group');

-- vault_members
create policy members_select on public.vault_members for select to authenticated
  using (vault_id in (select public.my_vault_ids()));

-- invites
create policy invites_select on public.invites for select to authenticated using (
  vault_id in (select public.my_vault_ids()) or email = (auth.jwt() ->> 'email')
);
create policy invites_insert on public.invites for insert to authenticated
  with check (public.is_group_owner(vault_id) and invited_by = auth.uid());
create policy invites_delete on public.invites for delete to authenticated
  using (public.is_vault_owner(vault_id));

-- projects
create policy projects_select on public.projects for select to authenticated
  using (vault_id in (select public.my_vault_ids()));
create policy projects_insert on public.projects for insert to authenticated with check (
  vault_id in (select public.my_vault_ids())
  and updated_by = auth.uid()
  and version = 1
  and key_generation = public.vault_generation(vault_id)
);
create policy projects_update on public.projects for update to authenticated
  using (vault_id in (select public.my_vault_ids()))
  with check (
    vault_id in (select public.my_vault_ids())
    and updated_by = auth.uid()
    and key_generation = public.vault_generation(vault_id)
  );
create policy projects_delete on public.projects for delete to authenticated
  using (vault_id in (select public.my_vault_ids()));

create function public.projects_bump() returns trigger
language plpgsql set search_path = '' as $$
begin
  if new.vault_id <> old.vault_id or new.id <> old.id then
    raise exception 'a project cannot move to a different vault';
  end if;
  new.version := old.version + 1;
  new.updated_at := now();
  return new;
end $$;
create trigger projects_bump before update on public.projects
  for each row execute function public.projects_bump();

create function public.create_vault(p_kind text, p_name_sealed text, p_wrapped_key text) returns uuid
language plpgsql security definer set search_path = '' as $$
declare v uuid;
begin
  if auth.uid() is null then raise exception 'sign in first'; end if;
  insert into public.vaults (kind, owner_id, name_sealed) values (p_kind, auth.uid(), p_name_sealed) returning id into v;
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (v, auth.uid(), 'owner', p_wrapped_key, 1);
  return v;
end $$;

create function public.accept_invite(p_invite uuid, p_user uuid, p_wrapped_key text) returns void
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
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (inv.vault_id, p_user, 'member', p_wrapped_key, gen);
  delete from public.invites where id = p_invite;
end $$;

create function public.rotate_vault(p_vault uuid, p_remove uuid, p_keys jsonb, p_projects jsonb) returns int
language plpgsql security definer set search_path = '' as $$
declare gen int; owner uuid; k jsonb; p jsonb; changed int;
begin
  select owner_id, key_generation into owner, gen from public.vaults where id = p_vault for update;
  if not found or owner <> auth.uid() then raise exception 'only the owner can remove a member'; end if;
  if p_remove = owner then raise exception 'the owner cannot be removed'; end if;
  delete from public.vault_members where vault_id = p_vault and user_id = p_remove;
  if not found then raise exception 'the user is not a member'; end if;
  gen := gen + 1;
  if (select count(*) from public.vault_members where vault_id = p_vault) <> jsonb_array_length(p_keys) then
    raise exception 'a new key is necessary for each remaining member';
  end if;
  for k in select * from jsonb_array_elements(p_keys) loop
    update public.vault_members set wrapped_key = k->>'wrapped_key', key_generation = gen
      where vault_id = p_vault and user_id = (k->>'user_id')::uuid;
    if not found then raise exception 'a key is for a user who is not a member'; end if;
  end loop;
  if exists (select 1 from public.vault_members where vault_id = p_vault and key_generation <> gen) then
    raise exception 'every remaining member must get a new key';
  end if;
  if (select count(*) from public.projects where vault_id = p_vault) <> jsonb_array_length(p_projects) then
    raise exception 'all projects of the vault must be encrypted again';
  end if;
  update public.vaults set key_generation = gen where id = p_vault;
  for p in select * from jsonb_array_elements(p_projects) loop
    update public.projects set sealed = p->>'sealed', key_generation = gen, updated_by = auth.uid()
      where id = (p->>'id')::uuid and vault_id = p_vault and version = (p->>'version')::int;
    get diagnostics changed = row_count;
    if changed <> 1 then raise exception 'a project changed during the removal; try again'; end if;
  end loop;
  return gen;
end $$;

-- function privileges: signed-in users only (RLS policies call the helpers as the user)
revoke execute on function
  public.create_vault, public.accept_invite, public.rotate_vault,
  public.my_vault_ids, public.is_vault_owner, public.is_group_owner, public.vault_generation,
  public.projects_bump
  from anon, public;
grant execute on function
  public.create_vault, public.accept_invite, public.rotate_vault,
  public.my_vault_ids, public.is_vault_owner, public.is_group_owner, public.vault_generation
  to authenticated;

-- table privileges: vaults and vault_members change only through the functions above,
-- so this does not depend on a missing policy alone
revoke all on public.profiles, public.vaults, public.vault_members, public.invites, public.projects from anon;
revoke insert, update on public.vaults, public.vault_members from authenticated;
revoke delete on public.vault_members from authenticated;
revoke truncate, references, trigger on
  public.profiles, public.vaults, public.vault_members, public.invites, public.projects
  from authenticated;
