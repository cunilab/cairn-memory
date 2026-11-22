import type { Metadata } from "next";
import { LoginForm } from "./_components/login-form";

export default function LoginPage() {
  return (
    <main className="flex min-h-svh w-full flex-1 items-center justify-center bg-background p-4 md:p-10">
      <div className="w-full max-w-sm md:max-w-4xl">
        <LoginForm />
      </div>
    </main>
  );
}

export const metadata: Metadata = { title: "Sign in" };
