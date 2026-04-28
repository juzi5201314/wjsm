try {
  try {
    console.log("inner");
    return 1;
  } finally {
    console.log("inner finally");
  }
} finally {
  console.log("outer finally");
}
