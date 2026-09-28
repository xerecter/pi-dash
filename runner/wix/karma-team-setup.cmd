@echo off
REM karma-team post-install sign-in helper.
REM
REM Launched from the MSI ExitDialog checkbox, as the *installing user*
REM rather than the Windows Installer service. That distinction matters:
REM `karma-team auth login` writes the machine token and workspace binding under
REM the user's profile, so running it as SYSTEM would file the credentials
REM against the wrong account.
REM
REM Exists as a .cmd rather than invoking karma-team.exe directly so the console
REM window survives the process exiting; otherwise a failed login flashes and
REM disappears before the user can read it.
setlocal

echo ==^> Starting karma-team sign-in...
echo.

"%~dp0karma-team.exe" auth login
set "_KARMA_TEAM_RC=%ERRORLEVEL%"

echo.
if not "%_KARMA_TEAM_RC%"=="0" (
  echo Sign-in did not complete ^(exit code %_KARMA_TEAM_RC%^).
  echo You can retry at any time with:
  echo.
  echo     karma-team auth login
  echo.
) else (
  echo Next: register a runner for a project with:
  echo.
  echo     karma-team runner add --project ^<PROJECT^>
  echo.
)

pause
endlocal
