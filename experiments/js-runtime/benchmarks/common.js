(() => {
  const results = [];

  function measure(name, operation) {
    for (let i = 0; i < 5; i++) operation();
    const start = Date.now();
    let checksum;
    for (let i = 0; i < 10; i++) checksum = operation();
    results.push({ name, elapsed_ms: Date.now() - start, checksum });
  }

  measure("integer_loop_500000", () => {
    let total = 0;
    for (let i = 0; i < 500000; i++) total += i;
    return total;
  });

  measure("object_array_50000", () => {
    const values = [];
    for (let i = 0; i < 50000; i++) values.push({ x: i, y: i * 2 });
    return values.length;
  });

  measure("string_concat_25000", () => {
    let value = "";
    for (let i = 0; i < 25000; i++) value += String(i);
    return value.length;
  });

  return JSON.stringify(results);
})()
