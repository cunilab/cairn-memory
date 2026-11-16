import type { Metadata } from "next";
import { Devices } from "@/components/devices";

export default function DevicesPage() {
  return <Devices />;
}

export const metadata: Metadata = { title: "Devices" };
