extern void consume(float first, float second, double third);
extern void consume_other(float first, float second, double third);

// The unknown-address store exercises the literal alias query between equal loads.
float literals_across_store(float *target, float input) {
    float before = input * 0.01f;
    *target = before;
    return input + 0.01f;
}

// The helper conversion creates a live range whose order depends on the flag.
void constant_arguments(float *values) {
    consume(values[0], 1.25f, values[1]);
}

void zero_arguments() {
    consume(-0.0f, 0.01f, 3.75);
}

// The conversion emits a compiler helper, exposing inherited helper masks.
int double_to_integer(double value) {
    return value;
}

long long double_to_wide_integer(double value) {
    return value;
}

void arguments_after_helpers(float input) {
    consume(input, 0.01f, 3.75);
}

// One stable bit selector must apply to both equal constants in this function.
void repeated_constant_arguments(float *values) {
    consume(values[0], 1.25f, values[1]);
    consume(values[2], 1.25f, values[3]);
}

// Non-simple constants take the full annotation path before returning.
void pooled_constant_arguments(float *values) {
    consume(values[0], 0.01f, values[1]);
}

// Optimization creates fresh literal nodes after the annotation pass.
void propagated_constant_arguments(float *values) {
    float local = 1.25f;
    consume(values[0], local, values[1]);
}

// The same value needs different policies at distinct named call targets.
void split_callee_arguments(float *values) {
    consume(values[0], 1.25f, values[1]);
    consume_other(values[2], 1.25f, values[3]);
}

// O2 may lower an exact integral float through an integer-to-float conversion.
void integral_constant_arguments(float *values) {
    consume(values[0], 257.0f, values[1]);
}
