#!/usr/bin/env sh
# Run from the OpenMES backend directory after installing its Composer dependencies.
set -eu
umask 077
if [ "${MOM_OPENMES_LIFECYCLE:-0}" = 1 ] && [ ! -x "${MOM_OPENMES_ADAPTER_SMOKE:-}" ]; then
    echo "MOM_OPENMES_LIFECYCLE requires MOM_OPENMES_ADAPTER_SMOKE" >&2
    exit 1
fi
sandbox=$(mktemp -d)
server_pid=
cleanup() {
    if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; fi
    rm -rf "$sandbox"
}
trap cleanup EXIT
export APP_ENV=testing DB_CONNECTION=sqlite DB_DATABASE="$sandbox/database.sqlite"
export APP_KEY="base64:$(php -r 'echo base64_encode(random_bytes(32));')"
export OPENMES_TELEMETRY=false NIGHTWATCH_ENABLED=false CACHE_STORE=array SESSION_DRIVER=array
export QUEUE_CONNECTION=sync BROADCAST_CONNECTION=null BCRYPT_ROUNDS=4
export ADAPTER_FIXTURE="$sandbox/fixture.json"
touch "$DB_DATABASE"
php artisan migrate --force --no-interaction --quiet
php <<'PHP'
<?php
use App\Models\Line;
use App\Models\User;
use App\Models\WorkOrder;
use Database\Seeders\RolesAndPermissionsSeeder;
use Illuminate\Contracts\Console\Kernel;

require 'vendor/autoload.php';
$app = require 'bootstrap/app.php';
$app->make(Kernel::class)->bootstrap();
$app->make(RolesAndPermissionsSeeder::class)->run();
function createOperator(Line $line): User {
    $actor = User::factory()->create();
    $actor->assignRole('Operator');
    $actor->lines()->attach($line->id);
    return $actor;
}

$line = Line::factory()->create();
$otherLine = Line::factory()->create();
$actor = createOperator($line);
$witness = createOperator($otherLine);
$visible = WorkOrder::factory()->create(['line_id' => $line->id]);
$hidden = WorkOrder::factory()->create(['line_id' => $otherLine->id]);
$fixture = [
    'visible' => $visible->id,
    'hidden' => $hidden->id,
    'token' => $actor->createToken('adapter-http-sample')->plainTextToken,
    'witness_token' => $witness->createToken('adapter-http-witness')->plainTextToken,
];
if (getenv('MOM_OPENMES_LIFECYCLE') === '1') {
    $admin = User::factory()->create();
    $admin->assignRole('Admin');
    $fixture['accept_token'] = $admin->createToken('adapter-source-transition')->plainTextToken;
}
file_put_contents(getenv('ADAPTER_FIXTURE'), json_encode($fixture, JSON_THROW_ON_ERROR));
PHP
(cd public && exec php -S 127.0.0.1:18765 ../vendor/laravel/framework/src/Illuminate/Foundation/resources/server.php) > "$sandbox/server.log" 2>&1 &
server_pid=$!
if ! php <<'PHP'
<?php
$fixture = json_decode(file_get_contents(getenv('ADAPTER_FIXTURE')), true, flags: JSON_THROW_ON_ERROR);
function request(int $id, ?string $token, bool $accept = false): array {
    $headers = "Accept: application/json\r\n";
    if ($token !== null) {
        $headers .= "Authorization: Bearer $token\r\n";
    }
    $context = stream_context_create(['http' => [
        'method' => $accept ? 'POST' : 'GET',
        'header' => $headers, 'ignore_errors' => true, 'timeout' => 3, 'follow_location' => 0,
    ]]);
    $path = "http://127.0.0.1:18765/api/v1/work-orders/$id" . ($accept ? '/accept' : '');
    $stream = @fopen($path, 'r', false, $context);
    if ($stream === false) {
        return [0, null];
    }
    $headers = stream_get_meta_data($stream)['wrapper_data'];
    $body = stream_get_contents($stream);
    fclose($stream);
    return [(int) explode(' ', $headers[0])[1], json_decode($body, true)];
}
for ($attempt = 0; $attempt < 30; ++$attempt) {
    [$status] = request($fixture['visible'], null);
    if ($status !== 0) {
        break;
    }
    usleep(200000);
}
foreach ([
    ['anonymous', $fixture['visible'], null, 401],
    ['assigned_line', $fixture['visible'], $fixture['token'], 200],
    ['other_line', $fixture['hidden'], $fixture['token'], 403],
    ['witness_assigned_line', $fixture['hidden'], $fixture['witness_token'], 200],
    ['witness_other_line', $fixture['visible'], $fixture['witness_token'], 403],
    ['missing', 999999, $fixture['token'], 404],
] as [$name, $id, $token, $expected]) {
    [$status, $result] = request($id, $token);
    if ($status !== $expected) {
        throw new RuntimeException("$name: expected HTTP $expected, received $status");
    }
    $evidence = ['case' => $name, 'http' => $status];
    if ($status === 200) {
        $data = $result['data'] ?? [];
        if (($data['id'] ?? null) !== $id || ($data['status'] ?? null) !== 'PENDING'
            || !isset($data['order_no'], $data['updated_at'])) {
            throw new RuntimeException('Unexpected work order response');
        }
        $evidence['work_order'] = array_intersect_key($data, array_flip(['id', 'order_no', 'status', 'updated_at']));
    }
    echo json_encode($evidence, JSON_THROW_ON_ERROR), PHP_EOL;
}

function checkAdapter(string $adapter, int $visible, int $hidden, string $token, string $expected): void {
    $env = getenv();
    unset($env['MOM_ODOO_WITNESS_TOKEN']);
    $env['MOM_SAMPLE_TOKEN'] = $token;
    $process = proc_open(
        [$adapter, 'openmes', 'http://127.0.0.1:18765', (string) $visible, (string) $hidden, $expected],
        [STDIN, STDOUT, STDERR],
        $pipes,
        null,
        $env,
    );
    if (!is_resource($process) || proc_close($process) !== 0) {
        throw new RuntimeException("OpenMES Rust Adapter smoke check failed for $expected");
    }
}

$adapter = getenv('MOM_OPENMES_ADAPTER_SMOKE');
if ($adapter !== false) {
    if (!is_file($adapter) || !is_executable($adapter)) {
        throw new RuntimeException('MOM_OPENMES_ADAPTER_SMOKE must name an executable');
    }
    $visible = $fixture['visible'];
    $hidden = $fixture['hidden'];
    checkAdapter($adapter, $visible, $hidden, $fixture['token'], 'PENDING');
    checkAdapter($adapter, $hidden, $visible, $fixture['witness_token'], 'PENDING');

    if (isset($fixture['accept_token'])) {
        [$status] = request($visible, $fixture['accept_token'], accept: true);
        if ($status !== 200) {
            throw new RuntimeException("OpenMES acceptance expected HTTP 200, received $status");
        }
        checkAdapter($adapter, $visible, $hidden, $fixture['token'], 'ACCEPTED');
        checkAdapter($adapter, $hidden, $visible, $fixture['witness_token'], 'PENDING');
        echo "OpenMES lifecycle: PENDING -> ACCEPTED confirmed by the read-only adapter", PHP_EOL;
    }
}
PHP
then
    cat "$sandbox/server.log" >&2
    exit 1
fi
