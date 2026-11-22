#!/usr/bin/env sh
# Run from the OpenMES backend directory after installing its Composer dependencies.
set -eu
umask 077
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
$actor = User::factory()->create();
$actor->assignRole('Operator');
$line = Line::factory()->create();
$actor->lines()->attach($line->id);
$visible = WorkOrder::factory()->create(['line_id' => $line->id]);
$hidden = WorkOrder::factory()->create(['line_id' => Line::factory()->create()->id]);
file_put_contents(getenv('ADAPTER_FIXTURE'), json_encode([
    'visible' => $visible->id,
    'hidden' => $hidden->id,
    'token' => $actor->createToken('adapter-http-sample')->plainTextToken,
], JSON_THROW_ON_ERROR));
PHP
(cd public && exec php -S 127.0.0.1:18765 ../vendor/laravel/framework/src/Illuminate/Foundation/resources/server.php) > "$sandbox/server.log" 2>&1 &
server_pid=$!
if ! php <<'PHP'
<?php
$fixture = json_decode(file_get_contents(getenv('ADAPTER_FIXTURE')), true, flags: JSON_THROW_ON_ERROR);
function request(int $id, ?string $token): array {
    $headers = "Accept: application/json\r\n";
    if ($token !== null) {
        $headers .= "Authorization: Bearer $token\r\n";
    }
    $context = stream_context_create(['http' => [
        'header' => $headers, 'ignore_errors' => true, 'timeout' => 3, 'follow_location' => 0,
    ]]);
    $stream = @fopen("http://127.0.0.1:18765/api/v1/work-orders/$id", 'r', false, $context);
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
PHP
then
    cat "$sandbox/server.log" >&2
    exit 1
fi
