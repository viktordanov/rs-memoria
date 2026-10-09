const apiUrl = process.env.API_URL ?? "http://localhost:8080";

export async function checkout(book: string): Promise<void> {
  await fetch(`${apiUrl}/orders`, {
    method: "POST",
    body: JSON.stringify({ book }),
  });
}
