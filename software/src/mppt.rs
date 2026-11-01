
/// Perturb and Observe MPPT
pub struct Mppt {
    prev_power: f32,
    duty: f32,
    step: f32,
    direction: f32, // +1 or -1
}

impl Mppt {
    pub fn new(initial_duty: f32, step: f32) -> Self {
        Self {
            prev_power: 0.0,
            duty: initial_duty,
            step,
            direction: 1.0,
        }
    }

    pub fn update(&mut self, voltage: f32, current: f32) -> f32 {
        let power = voltage * current;

        if power < self.prev_power {
            // reverse direction
            self.direction = -self.direction;
        }

        self.duty += self.direction * self.step;
        self.duty = self.duty.clamp(0.05, 0.95);

        self.prev_power = power;

        self.duty
    }

    pub fn set_duty(&mut self, duty: f32) {
        self.duty = duty
    }
}