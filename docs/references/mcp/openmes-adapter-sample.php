<?php

namespace Tests\Feature\Api;

use App\Models\Line;
use App\Models\User;
use App\Models\WorkOrder;
use Database\Seeders\RolesAndPermissionsSeeder;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\Auth;
use Tests\TestCase;

class AdapterSampleSmokeTest extends TestCase
{
    use RefreshDatabase;

    public function test_detail_with_line_scope_and_bearer_authentication(): void
    {
        $this->seed(RolesAndPermissionsSeeder::class);
        $actor = User::factory()->create();
        $actor->assignRole('Operator');
        $line = Line::factory()->create();
        $otherLine = Line::factory()->create();
        $actor->lines()->attach($line->id);
        $visible = WorkOrder::factory()->create(['line_id' => $line->id]);
        $hidden = WorkOrder::factory()->create(['line_id' => $otherLine->id]);
        $token = $actor->createToken('adapter-sample')->plainTextToken;

        $this->getJson("/api/v1/work-orders/{$visible->id}")->assertUnauthorized();
        Auth::forgetGuards();
        $response = $this->withToken($token)
            ->getJson("/api/v1/work-orders/{$visible->id}")
            ->assertOk()
            ->assertJsonPath('data.id', $visible->id)
            ->assertJsonPath('data.status', WorkOrder::STATUS_PENDING)
            ->assertJsonStructure(['data' => ['id', 'order_no', 'status', 'updated_at']]);
        Auth::forgetGuards();
        $this->getJson("/api/v1/work-orders/{$hidden->id}")->assertForbidden();
        Auth::forgetGuards();
        $this->getJson('/api/v1/work-orders/999999')->assertNotFound();
        fwrite(STDOUT, json_encode(array_intersect_key($response->json('data'), array_flip(['id','order_no','status','updated_at']))) . PHP_EOL);
    }
}
