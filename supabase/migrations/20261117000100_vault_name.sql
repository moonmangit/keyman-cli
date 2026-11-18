-- The owner of a group may change the sealed group name, and nothing else on a vault.
-- (The first migration already revoked insert and update on vaults. The revoke below
-- keeps the order safe if that changes: the final privilege is one column.)
revoke update on public.vaults from anon, authenticated;
grant update (name_sealed) on public.vaults to authenticated;

create policy vaults_update_name on public.vaults for update to authenticated
  using (owner_id = auth.uid() and kind = 'group')
  with check (owner_id = auth.uid() and kind = 'group');
