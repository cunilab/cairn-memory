import { handleProxy } from "@/lib/proxy";

type Context = { params: Promise<{ path: string[] }> };

async function proxy(request: Request, context: Context) {
  const { path } = await context.params;
  return handleProxy(request, path, process.env.CAIRN_API_URL ?? "http://127.0.0.1:3111");
}

export const GET = proxy;
export const POST = proxy;
