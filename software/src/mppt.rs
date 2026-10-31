/// Incremental Conductance algorithm for Boost converter
pub struct Mppt {
    prev_v: f32,
    prev_i: f32,
    duty: f32,
    step: f32,
    deadband: f32,
}

impl Mppt {
    pub const fn new(initial_duty: f32) -> Self {
        Self {
            prev_v: 0.0,
            prev_i: 0.0,
            duty: initial_duty,
            step: 0.002,
            deadband: 0.01,
        }
    }

    pub fn reset(&mut self) {
        self.duty = 0.0;
        self.prev_v = 0.0;
        self.prev_i = 0.0;
    }

    pub fn update(&mut self, v: f32, i: f32) -> f32 {
        let dv = v - self.prev_v;
        let di = i - self.prev_i;

        // Avoid division by zero
        if dv.abs() > 0.001 && v > 0.1 {
            let error = di / dv + i / v;

            if error > self.deadband {
                // Left of MPP
                self.duty -= self.step;
            } else if error < -self.deadband {
                // Right of MPP
                self.duty += self.step;
            }
        }

        self.duty = self.duty.clamp(0.05, 0.9);

        self.prev_v = v;
        self.prev_i = i;

        self.duty
    }
}
