import { describe, expect, it } from "vitest";
import { switchTeamLocation, teamLink } from "../workspace";
import {
  avatarUrl,
  getAvatarVersion,
  setAvatarVersion,
  setTeamAvatarVersion,
  teamAvatarUrl,
} from "../avatar-version";

// Bug 3: the URL is the team's single source of truth. These pure helpers
// are the link/switch surface the sidebar and switcher build on.
describe("team link + switch helpers", () => {
  it("teamLink carries the team param for list surfaces", () => {
    expect(teamLink("t1", "tasks")).toBe("/?team=t1");
    expect(teamLink("t1", "projects")).toBe("/projects?team=t1");
    expect(teamLink(undefined, "tasks")).toBe("/");
    expect(teamLink(undefined, "projects")).toBe("/projects");
  });

  it("switchTeamLocation rewrites the :teamId path segment in place", () => {
    expect(
      switchTeamLocation("/teams/a/settings", "", "b"),
    ).toEqual({ pathname: "/teams/b/settings", search: "" });
    expect(
      switchTeamLocation("/teams/a/goals", "?team=a", "b"),
    ).toEqual({ pathname: "/teams/b/goals", search: "?team=a" });
  });

  it("switchTeamLocation replaces ?team= on list surfaces", () => {
    expect(
      switchTeamLocation("/", "?team=a", "b"),
    ).toEqual({ pathname: "/", search: "team=b" });
    expect(
      switchTeamLocation("/projects", "?team=a&view=board", "b"),
    ).toEqual({ pathname: "/projects", search: "team=b&view=board" });
  });

  it("switchTeamLocation does NOT carry a project detail into the new team (Bug 2)", () => {
    // A project belongs to exactly one team: preserving its path rendered
    // the old team's data under the new team's ?team= (cross-team leak).
    expect(
      switchTeamLocation("/projects/p1", "?team=a", "b"),
    ).toEqual({ pathname: "/projects", search: "team=b" });
    expect(
      switchTeamLocation("/projects/p1/board", "?team=a", "b"),
    ).toEqual({ pathname: "/projects", search: "team=b" });
  });
});

// Bug 1: avatar cache-busting — a bump changes the URL and never goes
// backwards across successive uploads.
describe("avatar version store", () => {
  it("starts with a plain URL and bumps monotonically", () => {
    const id = "u-avatar-test";
    expect(getAvatarVersion(id)).toBe(0);
    expect(avatarUrl(id)).toBe(`/api/v1/users/${id}/avatar`);
    setAvatarVersion(id, 1000);
    expect(avatarUrl(id)).toBe(`/api/v1/users/${id}/avatar?v=1000`);
    // A second upload with a smaller timestamp still advances the version.
    setAvatarVersion(id, 500);
    expect(getAvatarVersion(id)).toBe(1001);
    expect(avatarUrl(id)).toBe(`/api/v1/users/${id}/avatar?v=1001`);
  });

  it("ignores unrelated storage payloads for other users", () => {
    const id = "u-avatar-other";
    expect(avatarUrl(id)).toBe(`/api/v1/users/${id}/avatar`);
    setAvatarVersion(id, 7);
    expect(avatarUrl(id)).toBe(`/api/v1/users/${id}/avatar?v=7`);
  });
});

describe("team avatar version store (Bug 8)", () => {
  it("bumps the team version without touching user entries", () => {
    const id = "t-avatar-team";
    expect(teamAvatarUrl(id)).toBe(`/api/v1/teams/${id}/avatar`);
    setTeamAvatarVersion(id, 1000);
    expect(teamAvatarUrl(id)).toBe(`/api/v1/teams/${id}/avatar?v=1000`);
    // A second upload always advances (never goes backwards).
    setTeamAvatarVersion(id, 400);
    expect(teamAvatarUrl(id)).toBe(`/api/v1/teams/${id}/avatar?v=1001`);
    // User and team namespaces are independent.
    expect(getAvatarVersion(id)).toBe(0);
  });
});
