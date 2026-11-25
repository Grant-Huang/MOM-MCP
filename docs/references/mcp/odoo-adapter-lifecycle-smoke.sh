#!/usr/bin/env bash
# Run from the MOM-MCP root with an isolated local Docker daemon.
set -euo pipefail
umask 077
name=mom-odoo-lifecycle-$$
db=$name-db
network=$name-network
sandbox=$(mktemp -d)
sample=$sandbox/sample.json
cleanup() {
  docker rm -f -v "$name" "$db" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  rm -rf "$sandbox"
}
trap cleanup EXIT
image=${MOM_ODOO_TEST_IMAGE:-odoo@sha256:03a47921f9eef2c225ded1167fe7828465d2b3177883f30d173bb1c3c792949b}
export MOM_ODOO_ADAPTER_SMOKE="${MOM_ODOO_ADAPTER_SMOKE:-target/debug/examples/external_order_smoke}"
if [ ! -x "$MOM_ODOO_ADAPTER_SMOKE" ]; then
  echo 'Build external_order_smoke or set MOM_ODOO_ADAPTER_SMOKE' >&2
  exit 1
fi
docker network create "$network" >/dev/null
docker run -d --name "$db" --network "$network" -e POSTGRES_USER=odoo -e POSTGRES_PASSWORD=odoo postgres:16 >/dev/null
for attempt in $(seq 1 30); do
  if docker exec "$db" pg_isready -h 127.0.0.1 -U odoo >/dev/null 2>&1; then break; fi
  sleep 1
done
docker run -d --name "$name" --network "$network" -e HOST="$db" -e USER=odoo -e PASSWORD=odoo -p 127.0.0.1::8069 "$image" -d m3_sample -i mrp --without-demo=all >/dev/null
address=$(docker port "$name" 8069/tcp)
for attempt in $(seq 1 180); do
  if docker logs "$name" 2>&1 | grep 'Registry loaded' >/dev/null; then break; fi
  sleep 1
done
docker logs "$name" 2>&1 | grep 'Registry loaded' >/dev/null
docker exec -i "$name" odoo shell -d m3_sample --db_host="$db" --db_user=odoo --db_password=odoo --no-http 2>"$sandbox/seed.log" <<'PY' | sed -n 's/^M3_SAMPLE=//p' > "$sample"
from datetime import datetime, timedelta
import json
a = env.company
b = env['res.company'].create({'name': 'M3 Other Company'})
def record(company):
    scoped = env(context=dict(env.context, allowed_company_ids=[company.id]))
    if not scoped['stock.warehouse'].search_count([('company_id', '=', company.id)]):
        scoped['stock.warehouse'].create({'company_id': company.id, 'name': 'M3 Warehouse', 'code': 'M3WH'})
    product = scoped['product.product'].create({'name': 'M3 sample', 'is_storable': True, 'company_id': company.id})
    return scoped['mrp.production'].create({'product_id': product.id, 'product_qty': 1, 'company_id': company.id})
visible, hidden = record(a), record(b)
def token(company, login):
    actor = env['res.users'].with_context(no_reset_password=True).create({
        'name': login, 'login': login, 'company_id': company.id,
        'company_ids': [(6,0,[company.id])],
        'group_ids': [(6,0,[env.ref('base.group_user').id, env.ref('mrp.group_mrp_user').id])]
    })
    return env['res.users.apikeys'].with_user(actor).sudo()._generate(
        'rpc', 'M3 acceptance', datetime.now() + timedelta(hours=1))
key = token(a, 'm3-primary')
witness = token(b, 'm3-witness')
env.cr.commit()
print('M3_SAMPLE=' + json.dumps({'database':'m3_sample','token':key,'witness':witness,'visible':visible.id,'hidden':hidden.id}))
PY

python3 - "$sample" "$address" "$name" "$db" <<'PY'
import json, os, subprocess, sys
with open(sys.argv[1]) as f:
    sample = json.load(f)
cmd = [os.environ['MOM_ODOO_ADAPTER_SMOKE'], 'odoo', 'http://' + sys.argv[2],
       str(sample['visible']), str(sample['hidden']), 'draft']
env = dict(os.environ, MOM_ODOO_DATABASE=sample['database'], MOM_SAMPLE_TOKEN=sample['token'],
           MOM_ODOO_WITNESS_TOKEN=sample['witness'])
subprocess.run(cmd, env=env, check=True)
confirm = f"""
order = env['mrp.production'].browse({int(sample['visible'])})
assert order.state == 'draft'
order.action_confirm()
assert order.state == 'confirmed'
env.cr.commit()
"""
subprocess.run(
    ['docker', 'exec', '-i', sys.argv[3], 'odoo', 'shell', '-d', sample['database'],
     '--db_host=' + sys.argv[4], '--db_user=odoo', '--db_password=odoo', '--no-http'],
    input=confirm, text=True, check=True, stdout=subprocess.DEVNULL,
)
cmd[-1] = 'confirmed'
subprocess.run(cmd, env=env, check=True)
cmd[-1] = 'draft'
cmd[3:5] = reversed(cmd[3:5])
env['MOM_SAMPLE_TOKEN'], env['MOM_ODOO_WITNESS_TOKEN'] = env['MOM_ODOO_WITNESS_TOKEN'], env['MOM_SAMPLE_TOKEN']
subprocess.run(cmd, env=env, check=True)
print("Odoo lifecycle: draft -> confirmed verified through the read-only adapter")
PY
