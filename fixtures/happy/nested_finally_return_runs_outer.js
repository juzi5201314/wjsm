try {
  try {
    return 1;
  } finally {
    return 2;
  }
} finally {
  console.log("outer");
}
