"use client";

import { Fragment, Suspense, useState, type ReactNode } from "react";
import Link from "next/link";
import { PinIcon, SearchIcon } from "lucide-react";
import { AppSidebar } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { MemorySheet } from "@/components/memory-sheet";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Separator } from "@/components/ui/separator";
import { SidebarInset, SidebarProvider, SidebarTrigger } from "@/components/ui/sidebar";

export type Crumb = { label: string; href?: string };

export function ConsoleShell({
  crumbs,
  children,
}: {
  crumbs: Crumb[];
  children: ReactNode;
}) {
  const [paletteOpen, setPaletteOpen] = useState(false);
  const page = crumbs[crumbs.length - 1];
  const trail = crumbs.slice(0, -1);
  return (
    <SidebarProvider className="flex-1">
      <a
        href="#main"
        className="sr-only z-50 rounded-md bg-background px-3 py-2 text-sm focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus-visible:ring-3 focus-visible:ring-ring/50"
      >
        Skip to content
      </a>
      <AppSidebar />
      <SidebarInset id="main" className="min-w-0">
        <header className="sticky top-0 z-20 flex h-12 shrink-0 items-center gap-2 border-b bg-background/85 px-3 backdrop-blur supports-[backdrop-filter]:bg-background/70 md:px-4">
          <SidebarTrigger className="-ml-1" />
          <Separator orientation="vertical" className="mx-1 h-4 data-vertical:self-auto" />
          <Breadcrumb className="min-w-0 flex-1">
            <BreadcrumbList className="flex-nowrap">
              {trail.map((crumb) => (
                <Fragment key={crumb.label}>
                  <BreadcrumbItem className="hidden sm:inline-flex">
                    {crumb.href ? (
                      <BreadcrumbLink render={<Link href={crumb.href} />}>{crumb.label}</BreadcrumbLink>
                    ) : (
                      crumb.label
                    )}
                  </BreadcrumbItem>
                  <BreadcrumbSeparator className="hidden sm:block" />
                </Fragment>
              ))}
              <BreadcrumbItem className="min-w-0">
                <BreadcrumbPage className="truncate">{page.label}</BreadcrumbPage>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="hidden gap-2 sm:inline-flex"
            nativeButton={false}
            render={<Link href="/memories?focus=search" />}
          >
            <SearchIcon />
            Search memories
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="hidden w-36 justify-start gap-2 text-muted-foreground lg:inline-flex"
            onClick={() => setPaletteOpen(true)}
          >
            <span>Jump to</span>
            <Kbd className="ml-auto">⌘K</Kbd>
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="sm:hidden"
            aria-label="Search memories"
            nativeButton={false}
            render={<Link href="/memories?focus=search" />}
          >
            <SearchIcon />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="sm:hidden"
            aria-label="Pinned memories"
            nativeButton={false}
            render={<Link href="/memories?view=pinned" />}
          >
            <PinIcon />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="lg:hidden"
            aria-label="Jump to a page or project"
            onClick={() => setPaletteOpen(true)}
          >
            <SearchIcon />
          </Button>
        </header>
        <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
        <Suspense>
          <MemorySheet />
        </Suspense>
        <div className="mx-auto flex w-full min-w-0 max-w-6xl flex-1 flex-col gap-6 px-4 py-6 md:px-6 md:py-8">
          {children}
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
